//! The Player view: the page of the Library window that plays an Item. The video shows through a transparent
//! hole, from the child surface under the window (`video.rs`). The controls draw over it, and hide 3 seconds
//! after the last input. The view only draws the state of the playback controller (`player.rs`) and sends the
//! user actions to it: a Playback session input with `step`, or an mpv change such as the volume.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled, Subscription, Task, Window, canvas, div, px, relative,
};
use gpui_omarchy::{ActiveTheme as _, IconName, icon};

use crate::api::types::{Delivery, StreamProtocol};
use crate::api::wire;
use crate::click::Clickable as _;
use crate::filter_panel::bar_pill;
use crate::keys::Key;
use crate::playback::Input;
use crate::playback::tracks::{Kind, Track};
use crate::player::PlayerWindow;
use crate::video::Surface;
use crate::{artwork, detail};

/// The controls hide after this time with no input.
const CONTROLS_TIME: Duration = Duration::from_secs(3);
/// A click on the video waits this long for a second click, so that a double-click does not pause. It is the
/// double-click time of GPUI on Linux.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Left and Right seek this many seconds.
const SEEK_STEP: f64 = 10.0;
/// `z`, `x` and the delay buttons change the subtitle delay by this many seconds, as the mpv keys do.
const DELAY_STEP: f64 = 0.1;
/// Up, Down and one wheel notch change the volume by this many percent.
const VOLUME_STEP: f64 = 5.0;
/// GPUI on Linux gives 3 lines for one wheel notch.
const LINES_PER_NOTCH: f32 = 3.0;
/// The touchpad distance of one wheel notch.
const PIXELS_PER_NOTCH: f32 = 50.0;
/// The width of the seek thumbnail over the seek bar.
const THUMB_WIDTH: f32 = 192.0;

pub struct PlayerView {
    player: Entity<PlayerWindow>,
    video: Rc<Surface>,
    focus_handle: FocusHandle,
    /// The controls show.
    controls: bool,
    _hide: Task<()>,
    /// The pause of a single click on the video, until the double-click time ends.
    _click: Option<Task<()>>,
    /// The mouse position of the last move, so that only a real move shows the controls.
    mouse: Option<Point<Pixels>>,
    /// Wheel notches that did not change the volume yet.
    scrolled: f32,
    /// The bounds of the seek bar in the last frame, for a click on it.
    bar: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The mouse is over the seek bar, this many pixels from its left edge.
    hover: Option<f32>,
    /// The view made the window fullscreen, so it leaves fullscreen when it closes.
    fullscreen: bool,
    /// The picker that shows over the video.
    picker: Option<Picker>,
    /// The Playback info panel shows.
    info: bool,
    _player: Subscription,
}

/// The lists that the Player view shows over the video. A new list is one more variant, with its arms in `title`,
/// `rows` and `pick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum List {
    /// The Track picker for the audio.
    Audio,
    /// The Track picker for the subtitles, with Off at the top.
    Subtitles,
    /// The file versions and the steps of the Quality ladder.
    Quality,
    /// The chapters of the file that plays.
    Chapters,
}

/// What a pick of a row sends.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// A track id, or `None` for subtitles off.
    Track(Option<i64>),
    /// A file id.
    Version(String),
    /// A step label of the Quality ladder.
    Quality(String),
    /// The start of a chapter, in player time.
    Chapter(f64),
}

impl List {
    fn title(self) -> &'static str {
        match self {
            List::Audio => "Audio",
            List::Subtitles => "Subtitles",
            List::Quality => "Quality",
            List::Chapters => "Chapters",
        }
    }
}

/// An open list and its keyboard focus.
struct Picker {
    list: List,
    focus: usize,
    scroll: ScrollHandle,
}

/// One row of a picker.
pub struct Row {
    pub label: String,
    /// The second line, for example the codec and the channels.
    pub detail: String,
    pub tags: Vec<&'static str>,
    /// The row plays now.
    pub current: bool,
}

impl PlayerView {
    pub fn new(player: Entity<PlayerWindow>, video: Rc<Surface>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&player, |_, _, cx| cx.notify());
        Self {
            player,
            video,
            focus_handle: cx.focus_handle(),
            controls: true,
            _hide: Task::ready(()),
            _click: None,
            mouse: None,
            scrolled: 0.0,
            bar: Rc::default(),
            hover: None,
            fullscreen: false,
            picker: None,
            info: false,
            _player: observe,
        }
    }

    /// The view shows: it takes the keyboard focus and shows the controls.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.wake(cx);
    }

    /// The view closed. It leaves the fullscreen that it entered, and removes the last video frame once the
    /// page shows over it.
    pub fn closed(&mut self, window: &mut Window) {
        self._click = None;
        if std::mem::take(&mut self.fullscreen) && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        let video = self.video.clone();
        window.on_next_frame(move |_, _| video.hide());
    }

    /// Shows the controls, and hides them after `CONTROLS_TIME` with no more input. The input also tells the
    /// Playback session that the user is not away, for the Still watching prompt.
    fn wake(&mut self, cx: &mut Context<Self>) {
        self.step(Input::Active, cx);
        self.controls = true;
        self._hide = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CONTROLS_TIME).await;
            this.update(cx, |this, cx| {
                (this.controls, this.hover) = (false, None);
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn step(&mut self, input: Input, cx: &mut Context<Self>) {
        self.player.update(cx, |player, cx| player.step(input, cx));
    }

    fn add_volume(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.player.update(cx, |player, cx| player.add_volume(delta, cx));
    }

    fn add_sub_delay(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.player.update(cx, |player, cx| player.add_sub_delay(delta, cx));
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.player.update(cx, |player, cx| player.toggle_mute(cx));
    }

    fn toggle_fullscreen(&mut self, window: &mut Window) {
        self.fullscreen = !window.is_fullscreen();
        window.toggle_fullscreen();
    }

    /// Stops playback with the stop report. The Library window then shows the page where playback started.
    fn back(&mut self, cx: &mut Context<Self>) {
        self.player.update(cx, |player, cx| player.back(cx));
    }

    /// Esc leaves fullscreen, then goes Back. A picker or a panel over the controls must close first: check it
    /// here, before fullscreen. The Up Next card is cancelled before fullscreen too.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_some() || std::mem::take(&mut self.info) {
            cx.notify();
        } else if self.player.read(cx).up_next().is_some() {
            self.step(Input::CancelUpNext, cx);
        } else if window.is_fullscreen() {
            self.toggle_fullscreen(window);
        } else {
            self.back(cx);
        }
    }

    /// The rows of `list` and the value that a pick of each one sends.
    fn rows(&self, list: List, cx: &Context<Self>) -> Vec<(Pick, Row)> {
        let player = self.player.read(cx);
        let tracks =
            |kind| track_rows(player.tracks(), kind).into_iter().map(|(id, row)| (Pick::Track(id), row)).collect();
        match list {
            List::Audio => tracks(Kind::Audio),
            List::Subtitles => tracks(Kind::Sub),
            List::Quality => match player.plan() {
                Some(plan) => quality_rows(player.versions(), plan, player.quality()),
                None => Vec::new(),
            },
            List::Chapters => match (player.version(), player.plan()) {
                (Some(version), Some(plan)) => {
                    chapter_rows(&version.chapters, plan.timeline.timeline_offset_seconds, player.position())
                }
                _ => Vec::new(),
            },
        }
    }

    /// Picks row `index` of the open list, and closes it.
    fn pick(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(list) = self.picker.take().map(|picker| picker.list) else {
            return;
        };
        cx.notify();
        let Some((pick, _)) = self.rows(list, cx).into_iter().nth(index) else {
            return;
        };
        match (list, pick) {
            (List::Audio, Pick::Track(id)) => self.step(Input::PickTrack { kind: Kind::Audio, id }, cx),
            (_, Pick::Track(id)) => self.step(Input::PickTrack { kind: Kind::Sub, id }, cx),
            (_, Pick::Version(file_id)) => self.step(Input::SwitchVersion(file_id), cx),
            (_, Pick::Quality(label)) => self.step(Input::PickQuality(label), cx),
            (_, Pick::Chapter(start)) => self.step(Input::SetPosition(start), cx),
        }
    }

    /// Opens `list` with the focus on the row that plays now, or closes it when it shows.
    fn toggle(&mut self, list: List, cx: &mut Context<Self>) {
        if self.picker.as_ref().is_some_and(|picker| picker.list == list) {
            self.picker = None;
        } else {
            let focus = self.rows(list, cx).iter().position(|(_, row)| row.current).unwrap_or(0);
            let scroll = ScrollHandle::new();
            scroll.scroll_to_item(focus);
            self.picker = Some(Picker { list, focus, scroll });
        }
        cx.notify();
    }

    /// The keys of an open picker: Up, Down and Enter. Returns false for a key that the picker does not use.
    fn picker_key(&mut self, key: &Key, cx: &mut Context<Self>) -> bool {
        let Some(picker) = &self.picker else {
            return false;
        };
        let (list, focus) = (picker.list, picker.focus);
        let last = self.rows(list, cx).len().saturating_sub(1);
        let focus = match key {
            Key::Up => focus.saturating_sub(1),
            Key::Down => (focus + 1).min(last),
            Key::Other("enter") => {
                self.pick(focus, cx);
                return true;
            }
            _ => return false,
        };
        if let Some(picker) = &mut self.picker {
            picker.focus = focus;
            picker.scroll.scroll_to_item(focus);
        }
        cx.notify();
        true
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.wake(cx);
        let key = Key::parse(&event.keystroke.key);
        if self.picker_key(&key, cx) {
            return cx.stop_propagation();
        }
        match key {
            Key::Other("a") => self.toggle(List::Audio, cx),
            Key::Other("s") => self.toggle(List::Subtitles, cx),
            Key::Other("v") => self.toggle(List::Quality, cx),
            Key::Other("i") => self.toggle_info(cx),
            Key::Other("c") => self.toggle(List::Chapters, cx),
            Key::Other("z") => self.add_sub_delay(-DELAY_STEP, cx),
            Key::Other("x") => self.add_sub_delay(DELAY_STEP, cx),
            Key::Other("space") => self.step(Input::PlayPause, cx),
            // Skip: the next shuffle pick or the next episode.
            Key::Other("n") => self.step(Input::Next, cx),
            Key::Left => self.step(Input::SeekBy(-SEEK_STEP), cx),
            Key::Right => self.step(Input::SeekBy(SEEK_STEP), cx),
            Key::Up => self.add_volume(VOLUME_STEP, cx),
            Key::Down => self.add_volume(-VOLUME_STEP, cx),
            Key::Other("f") => self.toggle_fullscreen(window),
            Key::Other("m") => self.toggle_mute(cx),
            // Takes the Skip prompt, Play now on the Up Next card, Continue on the Still watching prompt, or
            // retries after "Lost connection".
            Key::Other("enter") => self.step(Input::Action, cx),
            Key::Other("escape") => self.escape(window, cx),
            Key::Other("backspace" | "q") => self.back(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.mouse.replace(event.position) != Some(event.position) {
            self.wake(cx);
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.wake(cx);
        self.scrolled += match event.delta {
            ScrollDelta::Lines(lines) => lines.y / LINES_PER_NOTCH,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / PIXELS_PER_NOTCH,
        };
        let notches = self.scrolled.trunc();
        if notches != 0.0 {
            self.scrolled -= notches;
            self.add_volume(f64::from(notches) * VOLUME_STEP, cx);
        }
    }

    /// A click pauses or plays, after the double-click time. A double-click toggles fullscreen.
    fn click_video(&mut self, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.wake(cx);
        // A click next to an open picker or panel only closes it.
        if self.picker.take().is_some() || std::mem::take(&mut self.info) {
            return cx.notify();
        }
        if event.click_count() >= 2 {
            self._click = None;
            return self.toggle_fullscreen(window);
        }
        self._click = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DOUBLE_CLICK).await;
            this.update(cx, |this, cx| this.step(Input::PlayPause, cx)).ok();
        }));
    }

    fn toggle_info(&mut self, cx: &mut Context<Self>) {
        self.info = !self.info;
        cx.notify();
    }

    fn click_bar(&mut self, event: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.wake(cx);
        let (Some(bounds), Some(duration)) = (self.bar.get(), self.player.read(cx).duration()) else {
            return;
        };
        let x = f32::from(event.position().x - bounds.origin.x);
        self.step(Input::SetPosition(seek_target(x, f32::from(bounds.size.width), duration)), cx);
    }

    fn hover_bar(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(bounds) = self.bar.get() {
            self.hover = Some(f32::from(event.position.x - bounds.origin.x));
            cx.notify();
        }
    }

    /// The time over the hovered point of the seek bar, with its seek thumbnail when the server has them.
    fn preview(&self, x: f32, width: f32, duration: f64, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let player = self.player.read(cx);
        let position = seek_target(x, width, duration);
        let thumb = player.trickplay().zip(player.plan()).and_then(|(trickplay, plan)| {
            let (url, column, row) = thumb_tile(trickplay, position + plan.timeline.timeline_offset_seconds)?;
            let height = THUMB_WIDTH * trickplay.thumbnail_height as f32 / trickplay.thumbnail_width.max(1) as f32;
            // The whole sheet, moved so that only the tile of the thumbnail shows.
            let sheet = div()
                .absolute()
                .left(px(-(column as f32) * THUMB_WIDTH))
                .top(px(-(row as f32) * height))
                .w(px(trickplay.tile_columns as f32 * THUMB_WIDTH))
                .h(px(trickplay.tile_rows as f32 * height))
                .child(artwork::logo(url.to_string(), || div().into_any_element()));
            Some(div().relative().w(px(THUMB_WIDTH)).h(px(height)).overflow_hidden().rounded(px(6.)).child(sheet))
        });
        let left = (x - THUMB_WIDTH / 2.0).clamp(0.0, (width - THUMB_WIDTH).max(0.0));
        div()
            .absolute()
            .bottom(px(20.))
            .left(px(left))
            .w(px(THUMB_WIDTH))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .children(thumb)
            .child(div().px(px(6.)).rounded(px(4.)).bg(t.surface).text_color(t.bright).child(clock(position)))
    }

    /// A round button of the controls. A click does what its key does.
    fn button(
        &self,
        id: &'static str,
        name: IconName,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div().id(id).clickable().p(px(8.)).rounded_full().child(icon(name).size(px(20.))).on_click(cx.listener(
            move |this, _, window, cx| {
                this.wake(cx);
                click(this, window, cx);
            },
        ))
    }

    /// The top bar (Back and the title) and the bottom bar (seek bar, play and pause, the time, the volume and
    /// fullscreen). A new control is one more `button` in a bar. A picker or a panel is one more absolute child
    /// of the view.
    fn controls(&self, window: &Window, cx: &Context<Self>) -> [AnyElement; 2] {
        let t = cx.omarchy();
        let player = self.player.read(cx);
        let (position, duration) = (player.position(), player.duration());
        let fraction = match (position, duration) {
            (Some(position), Some(duration)) if duration > 0.0 => (position / duration).clamp(0.0, 1.0) as f32,
            _ => 0.0,
        };
        let time = match (position, duration) {
            (Some(position), Some(duration)) => format!("{} / {}", clock(position), clock(duration)),
            (Some(position), None) => clock(position),
            _ => String::new(),
        };
        let (play, mute) = (
            if player.paused() { IconName::Play } else { IconName::Pause },
            match player.volume() {
                Some((_, true)) => (IconName::VolumeX, "Muted".to_string()),
                Some((volume, false)) => (IconName::Volume2, format!("{volume:.0}%")),
                None => (IconName::Volume2, String::new()),
            },
        );
        let delay = player.sub_delay().filter(|delay| delay.abs() >= DELAY_STEP / 2.0).map(delay_label);
        let full = if window.is_fullscreen() { IconName::Minimize } else { IconName::Maximize };
        let shade = t.background.opacity(0.8);
        let top = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .px(px(16.))
            .py(px(12.))
            .flex()
            .items_center()
            .gap(px(12.))
            .bg(shade)
            .child(self.button("player-back", IconName::ChevronLeft, cx, |this, _, cx| this.back(cx)))
            .child(
                div()
                    .truncate()
                    .text_size(px(16.))
                    .text_color(t.bright)
                    .child(player.title().unwrap_or_default().to_string()),
            );
        // The Markers show as light ranges over the track, also over the played part.
        let markers = duration.into_iter().flat_map(|duration| {
            player.markers().iter().filter_map(move |marker| marker_span(marker.start, marker.end, duration))
        });
        let markers = markers.map(|(left, width)| {
            div().absolute().top_0().h_full().left(relative(left)).w(relative(width)).bg(t.bright.opacity(0.5))
        });
        let played = div().absolute().top_0().left_0().h_full().w(relative(fraction)).rounded_full().bg(t.accent);
        let track = div().relative().h(px(4.)).w_full().rounded_full().bg(t.inset).child(played).children(markers);
        let preview = self
            .hover
            .zip(self.bar.get())
            .zip(duration)
            .map(|((x, bounds), duration)| self.preview(x, f32::from(bounds.size.width), duration, cx));
        let bar = self.bar.clone();
        let seek = div()
            .id("seek-bar")
            .clickable()
            .relative()
            .h(px(16.))
            .flex()
            .items_center()
            .on_click(cx.listener(Self::click_bar))
            .on_mouse_move(cx.listener(Self::hover_bar))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.hover = None;
                    cx.notify();
                }
            }))
            .child(canvas(move |bounds, _, _| bar.set(Some(bounds)), |_, _, _, _| ()).absolute().size_full())
            .child(track);
        // The preview is outside the seek bar, so that the hover look of the bar does not dim it.
        let seek = div().relative().child(seek).children(preview);
        let row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(self.button("player-play", play, cx, |this, _, cx| this.step(Input::PlayPause, cx)))
            .when(player.can_go_next(), |row| {
                row.child(
                    self.button("player-next", IconName::SkipForward, cx, |this, _, cx| this.step(Input::Next, cx)),
                )
            })
            .child(div().text_color(t.secondary).child(time))
            .child(div().flex_1())
            .child(self.button("player-audio", IconName::AudioLines, cx, |this, _, cx| this.toggle(List::Audio, cx)))
            .child(
                self.button("player-subtitles", IconName::Captions, cx, |this, _, cx| this.toggle(List::Subtitles, cx)),
            )
            // The subtitle delay shows next to the subtitle button while it is not zero.
            .children(delay.map(|delay| div().text_color(t.secondary).child(delay)))
            .child(
                self.button("player-chapters", IconName::ListVideo, cx, |this, _, cx| this.toggle(List::Chapters, cx)),
            )
            .child(self.button("player-quality", IconName::Gauge, cx, |this, _, cx| this.toggle(List::Quality, cx)))
            .child(self.button("player-info", IconName::Info, cx, |this, _, cx| this.toggle_info(cx)))
            .child(self.button("player-mute", mute.0, cx, |this, _, cx| this.toggle_mute(cx)))
            .child(div().w(px(48.)).text_color(t.secondary).child(mute.1))
            .child(self.button("player-fullscreen", full, cx, |this, window, _| this.toggle_fullscreen(window)));
        let bottom = div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .px(px(16.))
            .pt(px(8.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .bg(shade)
            .child(seek)
            .child(row);
        [top.into_any_element(), bottom.into_any_element()]
    }

    /// The open picker over the video, above the bottom bar. A click on a row moves the focus to it and picks it,
    /// as Enter does.
    fn picker(&self, picker: &Picker, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let rows = self.rows(picker.list, cx);
        let empty = rows.is_empty();
        let rows = rows.into_iter().enumerate().map(|(i, (_, row))| {
            let tags = row.tags.into_iter().map(|tag| {
                div().px(px(6.)).rounded(px(4.)).bg(t.inset).text_size(px(11.)).text_color(t.secondary).child(tag)
            });
            div()
                .id(("picker-row", i))
                .clickable()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(8.))
                .when(i == picker.focus, |row| row.bg(t.selection))
                // The mark of the row that plays now.
                .child(div().w(px(16.)).when(row.current, |mark| mark.child(icon(IconName::Check).size(px(16.)))))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(
                            div().flex().flex_wrap().gap(px(6.)).text_color(t.bright).child(row.label).children(tags),
                        )
                        .when(!row.detail.is_empty(), |text| {
                            text.child(div().text_size(px(12.)).text_color(t.secondary).child(row.detail))
                        }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.wake(cx);
                    if let Some(picker) = &mut this.picker {
                        picker.focus = i;
                    }
                    this.pick(i, cx);
                }))
        });
        div()
            .id("picker")
            .absolute()
            .right(px(16.))
            .bottom(px(96.))
            .w(px(360.))
            .max_h(relative(0.6))
            .flex()
            .flex_col()
            .p(px(8.))
            .rounded(px(12.))
            .bg(t.surface)
            .child(div().px(px(12.)).py(px(6.)).text_color(t.secondary).child(picker.list.title()))
            .when(empty, |list| list.child(div().px(px(12.)).py(px(8.)).text_color(t.secondary).child("None")))
            .child(
                div()
                    .id("picker-rows")
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&picker.scroll)
                    .children(rows),
            )
            .when(picker.list == List::Subtitles, |list| list.child(self.delay(cx)))
    }

    /// The subtitle delay row below the Subtitles list: the value between a minus and a plus button. A click on
    /// a button does what `z` or `x` does.
    fn delay(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let value = delay_label(self.player.read(cx).sub_delay().unwrap_or_default());
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(12.))
            .pt(px(6.))
            .border_t_1()
            .border_color(t.inset)
            .child(div().flex_1().text_color(t.secondary).child("Delay"))
            .child(
                self.button("sub-delay-less", IconName::Minus, cx, |this, _, cx| this.add_sub_delay(-DELAY_STEP, cx)),
            )
            .child(div().w(px(56.)).flex().justify_center().text_color(t.bright).child(value))
            .child(self.button("sub-delay-more", IconName::Plus, cx, |this, _, cx| this.add_sub_delay(DELAY_STEP, cx)))
    }

    /// The Playback info panel, at the top right. A click on it does nothing, so it stays open.
    fn info(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let player = self.player.read(cx);
        let rows = player.plan().map(|plan| info_rows(plan, player.version(), player.quality())).unwrap_or_default();
        let rows = rows.into_iter().map(|(label, value)| {
            div()
                .flex()
                .gap(px(12.))
                .child(div().w(px(110.)).flex_none().text_color(t.secondary).child(label))
                .child(div().flex_1().text_color(t.bright).child(value))
        });
        div()
            .id("playback-info")
            .absolute()
            .right(px(16.))
            .top(px(64.))
            .w(px(420.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(t.surface)
            .text_size(px(13.))
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(div().pb(px(4.)).text_color(t.secondary).child("Playback info"))
            .children(rows)
    }

    /// The Skip prompt over the video. A click does what Enter does.
    fn prompt(&self, label: String, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        div()
            .id("skip-prompt")
            .clickable()
            .absolute()
            .right(px(32.))
            .bottom(px(112.))
            .px(px(18.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(t.surface)
            .text_color(t.bright)
            .child(label)
            .on_click(cx.listener(|this, _, _, cx| {
                this.wake(cx);
                this.step(Input::Action, cx);
            }))
    }
}

impl PlayerView {
    /// A pill of the Up Next card or the Still watching prompt. A click does what its key does.
    fn pill(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        bar_pill(id, label.into(), on, false, cx.omarchy()).on_click(cx.listener(move |this, _, _, cx| {
            this.wake(cx);
            click(this, cx);
        }))
    }

    /// The Up Next card: the seconds left, the next title, Play now (= Enter) and Cancel (= Esc). A click on the
    /// card does not pause.
    fn up_next(&self, left: u64, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let player = self.player.read(cx);
        let heading = if player.in_shuffle() { "Next in shuffle" } else { "Up next" };
        div()
            .id("up-next")
            .absolute()
            .right(px(32.))
            .bottom(px(112.))
            .w(px(340.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(t.surface)
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(12.)).text_color(t.secondary).child(format!("{heading} in {left} s")))
            .children(player.next_title().map(|title| div().truncate().text_color(t.bright).child(title.to_string())))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(self.pill("up-next-play", "Play now", true, cx, |this, cx| this.step(Input::Action, cx)))
                    .child(
                        self.pill("up-next-cancel", "Cancel", false, cx, |this, cx| this.step(Input::CancelUpNext, cx)),
                    ),
            )
    }

    /// The Still watching prompt in the middle of the view: Continue (= Enter) plays the next Item, Back (=
    /// Backspace) stops playback. A click next to the prompt does nothing.
    fn still_watching(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let next = self.player.read(cx).next_title().map(|title| format!("Next: {title}"));
        let prompt =
            div()
                .w(px(380.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(24.))
                .rounded(px(12.))
                .bg(t.surface)
                .child(div().text_size(px(18.)).text_color(t.bright).child("Are you still watching?"))
                .children(next.map(|next| div().truncate().text_color(t.secondary).child(next)))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(self.pill("still-watching-continue", "Continue", true, cx, |this, cx| {
                            this.step(Input::Action, cx)
                        }))
                        .child(self.pill("still-watching-back", "Back", false, cx, |this, cx| this.back(cx))),
                );
        div()
            .id("still-watching")
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(prompt)
    }
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let player = self.player.read(cx);
        let started = player.position().is_some();
        let prompt = Some(player.prompt().to_string()).filter(|label| !label.is_empty());
        let (up_next, still_watching) = (player.up_next(), player.still_watching());
        let video = self.video.clone();
        div()
            .id("player-view")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .size_full()
            .relative()
            .text_color(t.foreground)
            .font_family(t.font.clone())
            .text_size(px(14.))
            // The view draws no background once the video plays, so the video shows through. Before the first
            // frame, the theme background hides the desktop.
            .when(!started, |view| view.bg(t.background))
            .child(canvas(|_, _, _| (), move |bounds, _, window, _| video.place(bounds, window)).absolute().size_full())
            .child(div().id("video").absolute().size_full().on_click(cx.listener(Self::click_video)))
            .children(prompt.map(|label| self.prompt(label, cx)))
            .children(up_next.map(|left| self.up_next(left, cx)))
            .when(still_watching, |view| view.child(self.still_watching(cx)))
            .when(self.controls, |view| view.children(self.controls(window, cx)))
            .children(self.picker.as_ref().map(|picker| self.picker(picker, cx)))
            .when(self.info, |view| view.child(self.info(cx)))
    }
}

/// The Track picker rows of the tracks of `kind`, with the mpv track id that a pick sends. The subtitle list
/// starts with Off, whose pick sends `None`.
pub fn track_rows(tracks: &[Track], kind: Kind) -> Vec<(Option<i64>, Row)> {
    let of_kind = || tracks.iter().filter(move |track| track.kind == kind);
    let rows = of_kind().map(|track| {
        let row = Row { label: track.label(), detail: track.detail(), tags: track.tags(), current: track.selected };
        (Some(track.id), row)
    });
    let off = (kind == Kind::Sub).then(|| {
        let current = !of_kind().any(|track| track.selected);
        (None, Row { label: "Off".into(), detail: String::new(), tags: Vec::new(), current })
    });
    off.into_iter().chain(rows).collect()
}

/// The rows of the Quality list: the file versions when the Item has more than one, then the steps of the
/// Quality ladder when the plan offers more than one. A pick of a version sends its file id, a pick of a step its
/// label.
pub fn quality_rows(versions: &[wire::FileVersion], plan: &wire::PlaybackPlan, quality: &str) -> Vec<(Pick, Row)> {
    let versions = versions.iter().filter(|_| versions.len() > 1).map(|version| {
        let row = Row {
            label: detail::version_row(version),
            detail: version.edition_raw.clone().unwrap_or_default(),
            tags: vec!["Version"],
            current: *version.file_id == *plan.effective_media_file_id,
        };
        (Pick::Version(version.file_id.to_string()), row)
    });
    let ladder = &plan.available_qualities;
    let steps = ladder.iter().filter(|_| ladder.len() > 1).map(|step| {
        let row = Row {
            label: step_name(step),
            detail: step.bitrate_kbps.map(bitrate).unwrap_or_default(),
            tags: Vec::new(),
            current: step.label == quality,
        };
        (Pick::Quality(step.label.clone()), row)
    });
    versions.chain(steps).collect()
}

/// The rows of the Chapters list. `offset` is the source time where player time starts, and `position` the
/// player position. A pick sends the chapter start in player time; a chapter that starts before the stream
/// seeks to its start. The detail is the start in the file, and the row of the chapter that plays is current.
pub fn chapter_rows(chapters: &[wire::VersionChapter], offset: f64, position: Option<f64>) -> Vec<(Pick, Row)> {
    let source = position.map(|position| position + offset);
    chapters
        .iter()
        .enumerate()
        .map(|(i, chapter)| {
            let label = match chapter.title.trim() {
                "" => format!("Chapter {}", i + 1),
                title => title.to_string(),
            };
            let current = source.is_some_and(|source| chapter.start_seconds <= source && source < chapter.end_seconds);
            let row = Row { label, detail: clock(chapter.start_seconds), tags: Vec::new(), current };
            (Pick::Chapter((chapter.start_seconds - offset).max(0.0)), row)
        })
        .collect()
}

/// A subtitle delay as `+0.3 s`, `-1.2 s` or `0.0 s`.
pub fn delay_label(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round();
    if tenths == 0.0 { "0.0 s".to_string() } else { format!("{:+.1} s", tenths / 10.0) }
}

/// The name of a step of the Quality ladder: the display name of the server, else Original or the label.
fn step_name(step: &wire::AvailableQualityV3) -> String {
    match step.display_name.as_deref().filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None if step.preserves_source => "Original".into(),
        None => step.label.clone(),
    }
}

/// The rows of the Playback info panel: the file, the delivery, the codecs and the bitrate. An unknown value
/// shows as a dash.
pub fn info_rows(
    plan: &wire::PlaybackPlan,
    version: Option<&wire::FileVersion>,
    quality: &str,
) -> Vec<(&'static str, String)> {
    let (source, delivered) = (&plan.source, &plan.effective_recipe);
    let delivery = Delivery::from(plan.delivery.clone());
    // The server names each change that it makes to the stream.
    let changed = |name: &str| plan.transformations.iter().any(|t| t.executor == "server" && t.name == name);
    let codec = |codec: &Option<String>, transcoded: bool| {
        let how = if delivery == Delivery::OriginalHttp {
            "direct"
        } else if transcoded {
            "transcoded"
        } else {
            "copy"
        };
        codec.as_deref().filter(|c| !c.is_empty()).map(|c| format!("{} ({how})", c.to_uppercase())).unwrap_or_default()
    };
    let method = match &delivery {
        Delivery::OriginalHttp => "Direct play".to_string(),
        Delivery::ServerRemuxProgressive | Delivery::ServerRemuxHls => "Direct stream".to_string(),
        Delivery::ServerTranscodeHls => "Transcode".to_string(),
        Delivery::Unknown(other) => other.clone(),
    };
    let protocol = match StreamProtocol::from(plan.stream.protocol.clone()) {
        StreamProtocol::Hls => "HLS",
        _ => "Progressive",
    };
    let frame = |w: Option<i64>, h: Option<i64>| w.zip(h).map(|(w, h)| format!("{w}x{h}")).unwrap_or_default();
    let upper = |text: &Option<String>| text.as_deref().unwrap_or_default().to_uppercase();
    let file = version.and_then(|v| v.file_name.clone().or_else(|| v.file_path.clone())).unwrap_or_default();
    let container = source.container.clone().or_else(|| version.map(|v| v.container.clone()));
    let size = version.map(|v| detail::file_size(v.file_size)).unwrap_or_default();
    let step = plan.available_qualities.iter().find(|step| step.label == quality).map(step_name);
    let source_bitrate = source.bitrate_kbps.or(version.map(|v| v.bitrate)).filter(|kbps| *kbps > 0);
    let rows = [
        ("File", vec![file]),
        ("Container", vec![upper(&container), size]),
        ("Delivery", vec![method, protocol.into()]),
        ("Quality", vec![step.unwrap_or_else(|| quality.to_string())]),
        (
            "Video",
            vec![codec(&delivered.video_codec, changed("video_to_h264")), frame(delivered.width, delivered.height)],
        ),
        ("Audio", vec![codec(&delivered.audio_codec, changed("audio_to_aac"))]),
        ("Bitrate", vec![delivered.bitrate_kbps.filter(|kbps| *kbps > 0).map(bitrate).unwrap_or_default()]),
        (
            "Source video",
            vec![upper(&source.video_codec), frame(source.width, source.height), upper(&source.dynamic_range)],
        ),
        ("Source audio", vec![upper(&source.audio_codec), source.audio_layout.clone().unwrap_or_default()]),
        ("Source bitrate", vec![source_bitrate.map(bitrate).unwrap_or_default()]),
    ];
    rows.into_iter()
        .map(|(label, parts)| {
            let parts: Vec<String> = parts.into_iter().filter(|part| !part.trim().is_empty()).collect();
            (label, if parts.is_empty() { "—".to_string() } else { parts.join(" · ") })
        })
        .collect()
}

/// A bitrate as `25.6 Mbps`, or `800 kbps` under 1 Mbps.
pub fn bitrate(kbps: i64) -> String {
    if kbps >= 1000 { format!("{:.1} Mbps", kbps as f64 / 1000.0) } else { format!("{kbps} kbps") }
}

/// A time as `1:02:03`, or `2:03` under one hour.
pub fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

/// Where a Marker from `start` to `end` shows on a seek bar of a file of `duration` seconds: its left edge and
/// its width, as fractions of the bar. A Marker outside the file does not show.
pub fn marker_span(start: f64, end: f64, duration: f64) -> Option<(f32, f32)> {
    let (start, end) = (start.max(0.0), end.min(duration));
    (duration > 0.0 && end > start).then(|| ((start / duration) as f32, ((end - start) / duration) as f32))
}

/// The trickplay thumbnail of the source time `source`, in seconds: the URL of its sheet, and its column and row
/// on that sheet. A time outside the file shows the first or the last thumbnail.
pub fn thumb_tile(trickplay: &wire::WatchTrickplay, source: f64) -> Option<(&str, usize, usize)> {
    let (columns, per_sheet) = (trickplay.tile_columns, trickplay.tile_columns * trickplay.tile_rows);
    if trickplay.thumbnail_count <= 0 || trickplay.interval_ms <= 0 || per_sheet <= 0 {
        return None;
    }
    let thumbnail = ((source * 1000.0) as i64 / trickplay.interval_ms).clamp(0, trickplay.thumbnail_count - 1);
    let (sheet, tile) = (thumbnail / per_sheet, thumbnail % per_sheet);
    let url = &trickplay.sheets.iter().find(|s| s.index == sheet)?.url;
    Some((url, (tile % columns) as usize, (tile / columns) as usize))
}

/// The position that a click `x` pixels into a seek bar of `width` pixels seeks to.
pub fn seek_target(x: f32, width: f32, duration: f64) -> f64 {
    if width <= 0.0 {
        return 0.0;
    }
    f64::from((x / width).clamp(0.0, 1.0)) * duration
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Pick, chapter_rows, clock, delay_label, info_rows, marker_span, quality_rows, seek_target, thumb_tile,
        track_rows,
    };
    use crate::api::wire;
    use crate::playback::tracks::{Kind, parse};

    /// The transcode plan of the own replan fixture, with `fields` set.
    fn plan(fields: serde_json::Value) -> wire::PlaybackPlan {
        let body: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/own/playback_replan_failure_recovery.json")).unwrap();
        let mut plan = body["playback_plan"].clone();
        plan.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(plan).unwrap()
    }

    fn version(file_id: &str, resolution: &str) -> wire::FileVersion {
        serde_json::from_value(json!({
            "added_at": "2024-01-01T00:00:00.000Z", "bitrate": 25_600, "codec_audio": "eac3", "codec_video": "hevc",
            "container": "mkv", "duration": 7200, "file_id": file_id, "file_name": "Heat.1995.mkv",
            "file_size": 11_381_663_334_i64, "hdr": false, "resolution": resolution,
        }))
        .unwrap()
    }

    #[test]
    fn the_quality_list_has_the_versions_then_the_ladder_and_marks_what_plays() {
        let ladder = json!({"available_qualities": [
            {"label": "original", "display_name": "Original", "bitrate_kbps": 25_600, "preserves_source": true},
            {"label": "1080p-medium", "bitrate_kbps": 6000, "height": 1080, "preserves_source": false},
            {"label": "720p-low", "display_name": "720p Low", "bitrate_kbps": 800, "preserves_source": false},
        ]});
        let rows = quality_rows(&[version("42", "2160p"), version("43", "1080p")], &plan(ladder), "1080p-medium");
        let rows: Vec<_> = rows
            .iter()
            .map(|(pick, row)| (pick.clone(), row.label.as_str(), row.detail.as_str(), row.current))
            .collect();
        assert_eq!(rows[0].0, Pick::Version("42".into()));
        assert!(rows[0].1.starts_with("4K"), "{:?}", rows[0]);
        assert!(rows[0].3, "file 42 plays");
        assert_eq!((rows[1].0.clone(), rows[1].3), (Pick::Version("43".into()), false));
        assert_eq!(
            rows[2..],
            [
                (Pick::Quality("original".into()), "Original", "25.6 Mbps", false),
                (Pick::Quality("1080p-medium".into()), "1080p-medium", "6.0 Mbps", true),
                (Pick::Quality("720p-low".into()), "720p Low", "800 kbps", false),
            ]
        );
        // One version and one step give no choice.
        assert!(quality_rows(&[version("42", "1080p")], &plan(json!({})), "original").is_empty());
    }

    #[test]
    fn playback_info_shows_the_file_the_delivery_the_codecs_and_the_bitrate() {
        let transcode = plan(json!({
            "transformations": [{"executor": "server", "name": "video_to_h264", "recipe_version": "1", "validated_claims": []}],
        }));
        let rows = info_rows(&transcode, Some(&version("42", "1080p")), "original");
        let value = |label: &str| rows.iter().find(|(l, _)| *l == label).map(|(_, v)| v.as_str()).unwrap();
        assert_eq!(value("File"), "Heat.1995.mkv");
        assert_eq!(value("Container"), "MP4 · 10.6 GB");
        assert_eq!(value("Delivery"), "Transcode · HLS");
        assert_eq!(value("Quality"), "Original");
        assert_eq!(value("Video"), "H264 (transcoded) · 1920x1080");
        assert_eq!(value("Audio"), "AAC (copy)");
        assert_eq!(value("Bitrate"), "8.0 Mbps");
        assert_eq!(value("Source video"), "HEVC · 1920x1080 · SDR");
        assert_eq!(value("Source audio"), "AAC · stereo");
        assert_eq!(value("Source bitrate"), "8.0 Mbps");
        let direct = plan(
            json!({"delivery": "original_http", "stream": {"protocol": "http_progressive", "url": "/s", "headers": {}, "header_refresh": "none"}}),
        );
        let rows = info_rows(&direct, None, "original");
        let value = |label: &str| rows.iter().find(|(l, _)| *l == label).map(|(_, v)| v.as_str()).unwrap();
        assert_eq!(value("Delivery"), "Direct play · Progressive");
        assert_eq!(value("Video"), "H264 (direct) · 1920x1080");
        assert_eq!(value("File"), "—", "no version list yet");
    }

    fn chapter(index: i64, title: &str, start: f64, end: f64) -> wire::VersionChapter {
        serde_json::from_value(json!({
            "index": index, "title": title, "source": "embedded", "start_seconds": start, "end_seconds": end,
        }))
        .unwrap()
    }

    #[test]
    fn the_chapter_list_seeks_to_the_chapter_start_in_player_time_and_marks_the_chapter_that_plays() {
        let chapters =
            [chapter(0, "Opening", 0.0, 300.0), chapter(1, "", 300.0, 1500.0), chapter(2, "End", 1500.0, 1800.0)];
        // The stream starts 600 s into the file, and the source position is 700 s.
        let rows = chapter_rows(&chapters, 600.0, Some(100.0));
        let rows: Vec<_> = rows
            .iter()
            .map(|(pick, row)| (pick.clone(), row.label.as_str(), row.detail.as_str(), row.current))
            .collect();
        assert_eq!(
            rows,
            [
                (Pick::Chapter(0.0), "Opening", "0:00", false),
                (Pick::Chapter(0.0), "Chapter 2", "5:00", true),
                (Pick::Chapter(900.0), "End", "25:00", false),
            ]
        );
        let rows = chapter_rows(&chapters, 0.0, None);
        assert!(rows.iter().all(|(_, row)| !row.current), "no position yet");
        assert_eq!(rows[2].0, Pick::Chapter(1500.0));
    }

    #[test]
    fn the_subtitle_delay_shows_with_its_sign_and_one_decimal() {
        assert_eq!(delay_label(0.0), "0.0 s");
        assert_eq!(delay_label(0.30000000000000004), "+0.3 s");
        assert_eq!(delay_label(-1.26), "-1.3 s");
        assert_eq!(delay_label(-0.00001), "0.0 s");
    }

    #[test]
    fn the_subtitle_list_starts_with_off_and_marks_the_track_that_plays() {
        let tracks = parse(
            r#"[{"id":1,"type":"audio","lang":"eng","selected":true},{"id":2,"type":"audio","lang":"jpn"},
                {"id":1,"type":"sub","lang":"eng"},{"id":2,"type":"sub","lang":"jpn","selected":true}]"#,
        );
        let subtitles = track_rows(&tracks, Kind::Sub);
        let ids: Vec<_> = subtitles.iter().map(|(id, row)| (*id, row.label.as_str(), row.current)).collect();
        assert_eq!(ids, [(None, "Off", false), (Some(1), "eng", false), (Some(2), "jpn", true)]);
        let audio = track_rows(&tracks, Kind::Audio);
        let ids: Vec<_> = audio.iter().map(|(id, row)| (*id, row.current)).collect();
        assert_eq!(ids, [(Some(1), true), (Some(2), false)], "no Off row for audio");
        let off = track_rows(&parse(r#"[{"id":1,"type":"sub"}]"#), Kind::Sub);
        assert!(off[0].1.current, "Off plays when no subtitle track does");
    }

    #[test]
    fn the_time_shows_hours_only_from_one_hour() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(125.9), "2:05");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(clock(-1.0), "0:00");
    }

    #[test]
    fn a_marker_shows_at_its_fraction_of_the_duration_and_is_cut_to_the_file() {
        assert_eq!(marker_span(100.0, 300.0, 1000.0), Some((0.1, 0.2)));
        assert_eq!(marker_span(-50.0, 100.0, 1000.0), Some((0.0, 0.1)), "starts before the file");
        assert_eq!(marker_span(900.0, 1100.0, 1000.0), Some((0.9, 0.1)), "runs past the end");
        assert_eq!(marker_span(1100.0, 1200.0, 1000.0), None, "after the end");
        assert_eq!(marker_span(10.0, 20.0, 0.0), None, "no duration yet");
    }

    #[test]
    fn a_click_on_the_seek_bar_seeks_to_its_fraction_of_the_duration() {
        assert_eq!(seek_target(250.0, 1000.0, 7200.0), 1800.0);
        assert_eq!(seek_target(-5.0, 1000.0, 7200.0), 0.0, "left of the bar");
        assert_eq!(seek_target(1200.0, 1000.0, 7200.0), 7200.0, "right of the bar");
        assert_eq!(seek_target(10.0, 0.0, 7200.0), 0.0, "no bar yet");
    }

    #[test]
    fn a_hovered_time_shows_the_tile_of_its_thumbnail_on_its_sheet() {
        let trickplay: wire::WatchTrickplay =
            serde_json::from_str(include_str!("../tests/fixtures/silo/get_watch_trickplay_ok.json")).unwrap();
        let tile = |source| thumb_tile(&trickplay, source);
        // 10 s for each thumbnail, 10 x 10 thumbnails on each sheet, 1020 thumbnails.
        assert_eq!(tile(0.0), Some(("/api/v2/artwork/trickplay/42/9913/0.9913.jpg?exp=1767409445&sig=a", 0, 0)));
        assert_eq!(tile(125.0).map(|t| (t.1, t.2)), Some((2, 1)), "thumbnail 12");
        assert_eq!(tile(1234.5), Some(("/api/v2/artwork/trickplay/42/9913/1.9913.jpg?exp=1767409445&sig=b", 3, 2)));
        assert_eq!(tile(-5.0).map(|t| (t.1, t.2)), Some((0, 0)), "before the start");
        assert_eq!(
            tile(99_999.0),
            Some(("/api/v2/artwork/trickplay/42/9913/10.9913.jpg?exp=1767409445&sig=k", 9, 1)),
            "after the end shows the last thumbnail"
        );
        let mut empty = trickplay.clone();
        empty.thumbnail_count = 0;
        assert_eq!(thumb_tile(&empty, 10.0), None, "no thumbnails");
        let mut missing = trickplay.clone();
        missing.sheets.truncate(1);
        assert_eq!(thumb_tile(&missing, 1234.5), None, "no sheet for the thumbnail");
    }
}
