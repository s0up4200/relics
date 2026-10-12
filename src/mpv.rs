//! The mpv adapter. It is the only module that may hold a `libmpv2::Mpv` (clippy.toml
//! blocks the type everywhere else). It carries out the commands of the Playback
//! session and turns mpv events into Playback session inputs.
#![allow(clippy::disallowed_types)]

use std::ffi::{OsString, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use libmpv2::events::{Event, PropertyData};
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::{Format, Mpv, mpv_end_file_reason};

use crate::api::types::Marker;
use crate::playback::{Input, tracks};

pub use libmpv2::Error;

/// The app id of the Player window. Omarchy floats each window whose app id contains
/// `mpv`, so do not rename it.
pub const APP_ID: &str = "silo-player";

/// One mpv handle, which owns the Player window. Drop it to close the window.
pub struct Player {
    mpv: Arc<Mpv>,
    events: Option<JoinHandle<()>>,
}

impl Player {
    /// Starts mpv. `hwdec` is the hardware decoding mode, `auto-safe` or `no`. `options`
    /// are more mpv options, for example `vo=null` in the tests. A thread reads the mpv
    /// events and gives each one to `on_input` as a Playback session input. The last
    /// input is `Shutdown`.
    pub fn open(
        hwdec: &str,
        options: &[(&str, &str)],
        on_input: impl Fn(Input) + Send + 'static,
    ) -> Result<Self, Error> {
        let mpv = Mpv::with_initializer(|init| {
            init.set_option("wayland-app-id", APP_ID)?;
            init.set_option("fullscreen", true)?;
            init.set_option("hwdec", hwdec)?;
            init.set_option("load-scripts", false)?;
            // The Lua controller draws the controls, so the built-in controller and OSD bar are off.
            init.set_option("osc", false)?;
            init.set_option("osd-bar", false)?;
            // mpv keeps a track change for the next file. A track change must apply only to
            // the Playback session that plays now.
            init.set_option("reset-on-next-file", "aid,sid")?;
            // At the end of a file, mpv keeps the last frame and reports `eof-reached` instead of an end-file
            // event. The window stays after a failed load. So the "Lost connection" text shows in the window.
            init.set_option("keep-open", "yes")?;
            init.set_option("force-window", "yes")?;
            // Without this, the built-in ytdl hook opens a failed URL again without the headers.
            init.set_option("ytdl", false)?;
            // libmpv turns off the keys and the window close by default. q and the close
            // button must reach mpv, so that they end in a Shutdown event.
            init.set_option("input-default-bindings", true)?;
            init.set_option("input-vo-keyboard", true)?;
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

    /// Sends the theme colors, as `#RRGGBB`, to the Lua controller.
    pub fn send_theme(&self, accent: &str, foreground: &str, background: &str) -> Result<(), Error> {
        self.mpv.command("script-message", &["silo-theme", accent, foreground, background])
    }

    /// Sends the Markers, in player time, to the Lua controller as `kind:start:end,kind:start:end`.
    pub fn show_markers(&self, markers: &[Marker]) -> Result<(), Error> {
        let list: Vec<String> = markers
            .iter()
            .map(|marker| format!("{}:{}:{}", String::from(marker.kind.clone()), marker.start, marker.end))
            .collect();
        self.mpv.command("script-message", &["silo-markers", &list.join(",")])
    }

    /// Shows the Skip prompt with `label`. An empty label hides it.
    pub fn show_prompt(&self, label: &str) -> Result<(), Error> {
        self.mpv.command("script-message", &["silo-prompt", label])
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

    /// The volume in percent, and whether the audio is muted.
    pub fn volume(&self) -> Result<(f64, bool), Error> {
        Ok((self.mpv.get_property("volume")?, self.mpv.get_property("mute")?))
    }

    /// The handle that the render thread of the Player view draws the video with. Open the Player with
    /// `vo=libmpv`, and make the render context before the first load, else mpv plays no video.
    pub fn video(&self) -> Video {
        Video(self.mpv.clone())
    }

    /// Runs one mpv input command, for example `["keypress", "a"]`. The tests use it.
    pub fn command(&self, args: &[&str]) -> Result<(), Error> {
        let (name, args) = args.split_first().expect("a command has a name");
        self.mpv.command(name, args)
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

/// Finds `silo-controls.lua`, the Lua controller of the Player window. `var` reads an
/// environment variable. The search order is `$SILO_OMARCHY_DATA`, then
/// `$XDG_DATA_HOME/silo-omarchy`, then `silo-omarchy` in each `$XDG_DATA_DIRS` entry,
/// with the XDG defaults when a variable is not set.
pub fn find_controls(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // The XDG spec treats an empty variable as not set.
    let var = |name: &str| var(name).filter(|value| !value.is_empty());
    let data_home = var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| Path::new(&home).join(".local/share")));
    let data_dirs = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    var("SILO_OMARCHY_DATA")
        .map(PathBuf::from)
        .into_iter()
        .chain(data_home.into_iter().chain(std::env::split_paths(&data_dirs)).map(|dir| dir.join("silo-omarchy")))
        .map(|dir| dir.join("silo-controls.lua"))
        .find(|path| path.is_file())
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
            Some(Ok(Event::ClientMessage(message))) if message == ["silo-ready"] => Input::ControlsReady,
            Some(Ok(Event::ClientMessage(message))) if message == ["silo-action"] => Input::Action,
            // libmpv2 turns an end of file with an error into an error event. This is how a
            // failed load, such as a 401 on the stream URL, arrives.
            Some(Err(_)) => Input::LoadFailed,
            Some(Ok(_)) => continue,
        };
        on_input(input);
    }
}
