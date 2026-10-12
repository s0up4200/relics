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
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled, Subscription, Task, Window, canvas, div, px, relative,
};
use gpui_omarchy::{ActiveTheme as _, IconName, icon};

use crate::click::Clickable as _;
use crate::keys::Key;
use crate::playback::Input;
use crate::player::PlayerWindow;
use crate::video::Surface;

/// The controls hide after this time with no input.
const CONTROLS_TIME: Duration = Duration::from_secs(3);
/// A click on the video waits this long for a second click, so that a double-click does not pause. It is the
/// double-click time of GPUI on Linux.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Left and Right seek this many seconds.
const SEEK_STEP: f64 = 10.0;
/// Up, Down and one wheel notch change the volume by this many percent.
const VOLUME_STEP: f64 = 5.0;
/// GPUI on Linux gives 3 lines for one wheel notch.
const LINES_PER_NOTCH: f32 = 3.0;
/// The touchpad distance of one wheel notch.
const PIXELS_PER_NOTCH: f32 = 50.0;

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
    /// The view made the window fullscreen, so it leaves fullscreen when it closes.
    fullscreen: bool,
    _player: Subscription,
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
            fullscreen: false,
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

    /// Shows the controls, and hides them after `CONTROLS_TIME` with no more input.
    fn wake(&mut self, cx: &mut Context<Self>) {
        self.controls = true;
        self._hide = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CONTROLS_TIME).await;
            this.update(cx, |this, cx| {
                this.controls = false;
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
    /// here, before fullscreen.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            self.toggle_fullscreen(window);
        } else {
            self.back(cx);
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.wake(cx);
        match Key::parse(&event.keystroke.key) {
            Key::Other("space") => self.step(Input::PlayPause, cx),
            Key::Left => self.step(Input::SeekBy(-SEEK_STEP), cx),
            Key::Right => self.step(Input::SeekBy(SEEK_STEP), cx),
            Key::Up => self.add_volume(VOLUME_STEP, cx),
            Key::Down => self.add_volume(-VOLUME_STEP, cx),
            Key::Other("f") => self.toggle_fullscreen(window),
            Key::Other("m") => self.toggle_mute(cx),
            // Takes the Skip prompt, or retries after "Lost connection".
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
        if event.click_count() >= 2 {
            self._click = None;
            return self.toggle_fullscreen(window);
        }
        self._click = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DOUBLE_CLICK).await;
            this.update(cx, |this, cx| this.step(Input::PlayPause, cx)).ok();
        }));
    }

    fn click_bar(&mut self, event: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.wake(cx);
        let (Some(bounds), Some(duration)) = (self.bar.get(), self.player.read(cx).duration()) else {
            return;
        };
        let x = f32::from(event.position().x - bounds.origin.x);
        self.step(Input::SetPosition(seek_target(x, f32::from(bounds.size.width), duration)), cx);
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
        let bar = self.bar.clone();
        let seek = div()
            .id("seek-bar")
            .clickable()
            .relative()
            .h(px(16.))
            .flex()
            .items_center()
            .on_click(cx.listener(Self::click_bar))
            .child(canvas(move |bounds, _, _| bar.set(Some(bounds)), |_, _, _, _| ()).absolute().size_full())
            .child(track);
        let row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(self.button("player-play", play, cx, |this, _, cx| this.step(Input::PlayPause, cx)))
            .child(div().text_color(t.secondary).child(time))
            .child(div().flex_1())
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

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let player = self.player.read(cx);
        let started = player.position().is_some();
        let prompt = Some(player.prompt().to_string()).filter(|label| !label.is_empty());
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
            .when(self.controls, |view| view.children(self.controls(window, cx)))
    }
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

/// The position that a click `x` pixels into a seek bar of `width` pixels seeks to.
pub fn seek_target(x: f32, width: f32, duration: f64) -> f64 {
    if width <= 0.0 {
        return 0.0;
    }
    f64::from((x / width).clamp(0.0, 1.0)) * duration
}

#[cfg(test)]
mod tests {
    use super::{clock, marker_span, seek_target};

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
}
