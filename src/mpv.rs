//! The mpv adapter. It is the only module that may hold a `libmpv2::Mpv` or a `Player` (clippy.toml blocks the
//! types everywhere else). It carries out the commands of the Playback session and turns mpv events into
//! Playback session inputs. `InView` is the media of the controller (`player::Media`).
#![allow(clippy::disallowed_types)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::JoinHandle;

use futures::channel::mpsc::{self, UnboundedReceiver};
use libmpv2::events::{Event, PropertyData};
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::{Format, Mpv, mpv_end_file_reason};

use crate::playback::{Input, tracks};
use crate::player::{Handle, Media};
use crate::settings::{LocalSettings, SubtitleAppearance};
use crate::video::Surface;

/// The media of the Player view. Each handle plays with no window, and the render thread of the Player view
/// draws its video in the video surface of the Library window.
pub struct InView {
    pub video: Rc<Surface>,
    /// The state directory of a test session (`library::Session`), for example the live-run rig. mpv reads the
    /// Local settings there and writes its log to `mpv.log` there, and the media keys do not reach it.
    pub test_dir: Option<PathBuf>,
}

impl Media for InView {
    fn open(&self) -> Result<(Box<dyn Handle>, UnboundedReceiver<Input>), String> {
        // Each new handle reads the hardware decoding Local setting again.
        let settings = self.test_dir.clone().unwrap_or_else(LocalSettings::dir);
        let hwdec = LocalSettings::load(&settings).hwdec();
        let log = self.test_dir.as_ref().map(|dir| dir.join("mpv.log").to_string_lossy().into_owned());
        let mut options = vec![("vo", "libmpv")];
        options.extend(log.as_deref().map(|log| ("log-file", log)));
        let (tx, rx) = mpsc::unbounded();
        let player = Player::open(hwdec, &options, move |input| drop(tx.unbounded_send(input)))
            .map_err(|err| err.to_string())?;
        self.video.start(player.video())?;
        Ok((Box::new(Open { player, video: self.video.clone() }), rx))
    }

    fn media_keys(&self) -> bool {
        self.test_dir.is_none()
    }
}

/// A handle of `InView`.
struct Open {
    player: Player,
    video: Rc<Surface>,
}

impl Drop for Open {
    /// The render thread stops before mpv, and before the window can close its Wayland connection.
    fn drop(&mut self) {
        self.video.stop();
    }
}

impl Handle for Open {
    fn load(&self, url: &str, headers: &[(String, String)], start: f64, aid: Option<i64>) -> anyhow::Result<()> {
        Ok(self.player.load(url, headers, start, aid)?)
    }

    fn add_subtitle(&self, url: &str, title: &str, language: &str) -> anyhow::Result<()> {
        Ok(self.player.add_subtitle(url, title, language)?)
    }

    fn select_subtitle(&self, sid: Option<i64>) -> anyhow::Result<()> {
        Ok(self.player.select_subtitle(sid)?)
    }

    fn select_audio(&self, aid: i64) -> anyhow::Result<()> {
        Ok(self.player.select_audio(aid)?)
    }

    fn seek(&self, position: f64) -> anyhow::Result<()> {
        Ok(self.player.seek(position)?)
    }

    fn set_pause(&self, paused: bool) -> anyhow::Result<()> {
        Ok(self.player.set_pause(paused)?)
    }

    fn add_volume(&self, delta: f64) -> anyhow::Result<()> {
        Ok(self.player.add_volume(delta)?)
    }

    fn toggle_mute(&self) -> anyhow::Result<()> {
        Ok(self.player.toggle_mute()?)
    }

    fn volume(&self) -> anyhow::Result<(f64, bool)> {
        Ok(self.player.volume()?)
    }

    fn sub_delay(&self) -> anyhow::Result<f64> {
        Ok(self.player.sub_delay()?)
    }

    fn add_sub_delay(&self, delta: f64) -> anyhow::Result<()> {
        Ok(self.player.add_sub_delay(delta)?)
    }

    fn set_sub_delay(&self, seconds: f64) -> anyhow::Result<()> {
        Ok(self.player.set_sub_delay(seconds)?)
    }

    fn set_subtitle_appearance(&self, appearance: &SubtitleAppearance) -> anyhow::Result<()> {
        Ok(self.player.set_subtitle_appearance(appearance)?)
    }
}

pub use libmpv2::Error;

/// One mpv handle. The Player view draws its video (`video`). Drop it to stop playback.
pub struct Player {
    mpv: Arc<Mpv>,
    events: Option<JoinHandle<()>>,
}

impl Player {
    /// Starts mpv. `hwdec` is the hardware decoding mode, `auto-safe` or `no`. `options`
    /// are more mpv options, for example `vo=libmpv` for the Player view or `vo=null` in the tests. A thread reads the mpv
    /// events and gives each one to `on_input` as a Playback session input. The last
    /// input is `Shutdown`.
    pub fn open(
        hwdec: &str,
        options: &[(&str, &str)],
        on_input: impl Fn(Input) + Send + 'static,
    ) -> Result<Self, Error> {
        let mpv = Mpv::with_initializer(|init| {
            init.set_option("hwdec", hwdec)?;
            init.set_option("load-scripts", false)?;
            // The Player view draws the controls, so the built-in controller and OSD bar are off.
            init.set_option("osc", false)?;
            init.set_option("osd-bar", false)?;
            // mpv keeps a track change for the next file. A track change must apply only to
            // the Playback session that plays now.
            init.set_option("reset-on-next-file", "aid,sid")?;
            // At the end of a file, mpv keeps the last frame and reports `eof-reached` instead of an end-file
            // event. mpv stays open after a failed load, so the Player view can show "Lost connection".
            init.set_option("keep-open", "yes")?;
            // Without this, the built-in ytdl hook opens a failed URL again without the headers.
            init.set_option("ytdl", false)?;
            for (name, value) in options {
                init.set_option(name, *value)?;
            }
            Ok(())
        })?;
        mpv.observe_property("time-pos", Format::Double, 0)?;
        mpv.observe_property("duration", Format::Double, 0)?;
        mpv.observe_property("pause", Format::Flag, 0)?;
        mpv.observe_property("eof-reached", Format::Flag, 0)?;
        // As a string: observing `track-list` as a node panics the event thread in libmpv2.
        mpv.observe_property("track-list", Format::String, 0)?;
        let mpv = Arc::new(mpv);
        let events = std::thread::spawn({
            let mpv = mpv.clone();
            move || read_events(&mpv, on_input)
        });
        Ok(Self { mpv, events: Some(events) })
    }

    /// Opens `url` and plays it from `start` seconds, with no subtitles, and with the audio track `aid` when it is
    /// set. mpv sends `headers` on each media request of this file. They go in the per-file options of
    /// `loadfile`, because mpv copies `http-header-fields` only when it opens a file.
    pub fn load(&self, url: &str, headers: &[(String, String)], start: f64, aid: Option<i64>) -> Result<(), Error> {
        // A string list separates its items with commas, and a backslash escapes a comma in an item.
        let fields: Vec<String> = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}").replace('\\', "\\\\").replace(',', "\\,"))
            .collect();
        // The app picks the subtitles (`add_subtitle`), so mpv must not pick a track of the file by itself.
        let mut options = vec![
            option("http-header-fields", &fields.join(",")),
            option("start", &start.to_string()),
            option("sid", "no"),
        ];
        options.extend(aid.map(|aid| option("aid", &aid.to_string())));
        self.mpv.command("loadfile", &[url, "replace", "-1", &options.join(",")])
    }

    /// Adds the subtitle file at `url` to the file that plays, and shows it. mpv sends the headers of
    /// the file. The download runs in the background, so a slow server does not block the caller.
    pub fn add_subtitle(&self, url: &str, title: &str, language: &str) -> Result<(), Error> {
        self.mpv.command("async", &["sub-add", url, "select", title, language])
    }

    /// Shows the subtitle track `sid` of the file that plays. `None` hides the subtitles.
    pub fn select_subtitle(&self, sid: Option<i64>) -> Result<(), Error> {
        match sid {
            Some(sid) => self.mpv.set_property("sid", sid),
            None => self.mpv.set_property("sid", "no"),
        }
    }

    /// Plays the audio track `aid` of the file that plays.
    pub fn select_audio(&self, aid: i64) -> Result<(), Error> {
        self.mpv.set_property("aid", aid)
    }

    /// Seeks to `position`, in player time.
    pub fn seek(&self, position: f64) -> Result<(), Error> {
        self.mpv.command("seek", &[&position.to_string(), "absolute"])
    }

    /// Pauses or plays the file.
    pub fn set_pause(&self, paused: bool) -> Result<(), Error> {
        self.mpv.set_property("pause", paused)
    }

    /// Changes the volume by `delta` percent. mpv keeps it between 0 and `volume-max`.
    pub fn add_volume(&self, delta: f64) -> Result<(), Error> {
        self.mpv.command("add", &["volume", &delta.to_string()])
    }

    pub fn toggle_mute(&self) -> Result<(), Error> {
        self.mpv.command("cycle", &["mute"])
    }

    /// The subtitle delay in seconds. A positive delay shows the subtitles later.
    pub fn sub_delay(&self) -> Result<f64, Error> {
        self.mpv.get_property("sub-delay")
    }

    /// Changes the subtitle delay by `delta` seconds.
    pub fn add_sub_delay(&self, delta: f64) -> Result<(), Error> {
        self.mpv.command("add", &["sub-delay", &delta.to_string()])
    }

    /// Sets the subtitle delay to `seconds`. mpv keeps it from one file to the next.
    pub fn set_sub_delay(&self, seconds: f64) -> Result<(), Error> {
        self.mpv.set_property("sub-delay", seconds)
    }

    /// Sets the mpv options that draw plain-text subtitles, to match the look of the web player. mpv scales the
    /// sizes and margins from a window height of 720, like the web. Styled (ASS) subtitles keep their own styles.
    pub fn set_subtitle_appearance(&self, appearance: &SubtitleAppearance) -> Result<(), Error> {
        let font_size = match appearance.font_size.as_str() {
            "small" => "20",
            "medium" => "26",
            "xlarge" => "40",
            "xxlarge" => "48",
            _ => "32",
        };
        let style = appearance.background_style.as_str();
        let (border_style, back_color, shadow_offset) = match style {
            // The shadow offset is the margin of the background box.
            "box" => ("background-box", color(&appearance.background_color, appearance.background_opacity), "4"),
            // The black drop shadow of the web, at 90%.
            "shadow" => ("outline-and-shadow", color("#000000", 90), "2"),
            _ => ("outline-and-shadow", color("#000000", 0), "0"),
        };
        // mpv draws one outline, for the outline style and for Text outline.
        let outline_size = if style == "outline" || appearance.text_outline { "2" } else { "0" };
        // 7% and 12% of 720.
        let (align, margin) = match appearance.position.as_str() {
            "top" => ("top", "50"),
            "lower-third" => ("bottom", "86"),
            _ => ("bottom", "50"),
        };
        let options = [
            ("sub-font-size", font_size),
            ("sub-font", &appearance.font_family),
            ("sub-color", &color(&appearance.font_color, appearance.text_opacity)),
            ("sub-border-style", border_style),
            ("sub-back-color", &back_color),
            ("sub-shadow-offset", shadow_offset),
            ("sub-outline-size", outline_size),
            ("sub-outline-color", &color(&appearance.text_outline_color, 100)),
            ("sub-align-y", align),
            ("sub-margin-y", margin),
        ];
        for (name, value) in options {
            self.mpv.set_property(name, value)?;
        }
        Ok(())
    }

    /// The volume in percent, and whether the audio is muted.
    pub fn volume(&self) -> Result<(f64, bool), Error> {
        Ok((self.mpv.get_property("volume")?, self.mpv.get_property("mute")?))
    }

    /// The handle that the render thread of the Player view draws the video with. Open the Player with
    /// `vo=libmpv`, and make the render context before the first load, else mpv plays no video.
    pub fn video(&self) -> Video {
        Video(self.mpv.clone())
    }

    /// Reads an mpv property as text. The tests use it.
    pub fn property(&self, name: &str) -> Result<String, Error> {
        self.mpv.get_property(name)
    }
}

/// The mpv handle of the render thread of the Player view.
pub struct Video(Arc<Mpv>);

impl Video {
    /// Makes the OpenGL render context. Call it on the render thread, with the GL context current. Drop the
    /// render context on the same thread, before the Player.
    pub fn render_context<C: 'static>(
        &self,
        get_proc_address: fn(&C, &str) -> *mut c_void,
        gl: C,
        wl_display: *const c_void,
    ) -> Result<RenderContext<'_>, Error> {
        self.0.create_render_context([
            RenderParam::ApiType(RenderParamApiType::OpenGl),
            RenderParam::InitParams(OpenGLInitParams { get_proc_address, ctx: gl }),
            // VA-API decoding needs the Wayland display to share the frames with OpenGL.
            RenderParam::WaylandDisplay(wl_display),
        ])
    }
}

/// An mpv color `#AARRGGBB` from a `#rrggbb` color and an alpha in percent.
fn color(rgb: &str, percent: u64) -> String {
    format!("#{:02X}{}", (percent * 255 + 50) / 100, rgb.trim_start_matches('#').to_uppercase())
}

/// One `name=value` item of an mpv key-value list. The `%length%` prefix quotes the value,
/// so a comma or an equals sign in it is safe.
fn option(name: &str, value: &str) -> String {
    format!("{name}=%{}%{value}", value.len())
}

impl Drop for Player {
    fn drop(&mut self) {
        // After a Shutdown, mpv refuses the command, and the thread has already stopped.
        self.mpv.command("quit", &[]).ok();
        if let Some(events) = self.events.take() {
            events.join().ok();
        }
    }
}

fn read_events(mpv: &Mpv, on_input: impl Fn(Input)) {
    loop {
        let input = match mpv.wait_event(-1.0) {
            None => continue,
            Some(Ok(Event::Shutdown)) => {
                on_input(Input::Shutdown);
                return;
            }
            Some(Ok(Event::FileLoaded)) => Input::FileLoaded,
            // mpv ended a seek, or started the file. MPRIS sends `Seeked` with the new position.
            Some(Ok(Event::PlaybackRestart)) => match mpv.get_property::<f64>("time-pos") {
                Ok(position) => Input::Seeked(position),
                Err(_) => continue,
            },
            Some(Ok(Event::EndFile(mpv_end_file_reason::Eof))) => Input::EndOfFile,
            Some(Ok(Event::EndFile(mpv_end_file_reason::Error))) => Input::LoadFailed,
            Some(Ok(Event::PropertyChange { name, change, .. })) => match (name, change) {
                ("time-pos", PropertyData::Double(position)) => Input::Position(position),
                ("duration", PropertyData::Double(duration)) => Input::Duration(duration),
                ("pause", PropertyData::Flag(paused)) => Input::Paused(paused),
                ("eof-reached", PropertyData::Flag(true)) => Input::EndOfFile,
                ("track-list", PropertyData::Str(json)) => Input::Tracks(tracks::parse(json)),
                _ => continue,
            },
            // libmpv2 turns an end of file with an error into an error event. This is how a
            // failed load, such as a 401 on the stream URL, arrives.
            Some(Err(_)) => Input::LoadFailed,
            Some(Ok(_)) => continue,
        };
        on_input(input);
    }
}
