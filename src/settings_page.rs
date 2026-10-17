//! The Settings page. `,` or the Settings row at the bottom of the sidebar opens it.
//!
//! - The Server settings of the active Profile, in the groups of the web Settings pages. The page reads them
//!   again each time it opens. Each change writes at the `profile` scope at once. A failed write puts the old
//!   value back and shows one line of error text. The app keeps no queue of failed writes.
//! - This computer: the hardware decoding Local setting, and the "Check for new releases" Local setting of the
//!   Release notice.
//! - Accounts: each Server and Account pair with Sign out and Remove, and Add server below.
//!
//! A group with no visible rows does not show.
//!
//! j/k move between the rows. l or Right gives the next value, h or Left the previous one. Enter or a click
//! on a setting with more than two choices opens the list of its choices below the row. In the list j/k move,
//! Enter or a click picks the choice, and Escape, Backspace or h closes it. Enter on an On/Off setting toggles it.
//! On an Account row h/l choose Sign out or Remove, and Enter does it.
//!
//! The footer shows the build identity, and a link to the release page when a newer release exists.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};
use serde_json::Value;

use crate::api::Client;
use crate::choice_list::{Choice, ChoiceList, Outcome};
use crate::click::Clickable as _;
use crate::keys::Key;
use crate::pages::freshness::{Deps, Fresh, Reload};
use crate::pages::loading::{Failure, Loader};
use crate::release::ReleaseCheck;
use crate::settings::{DEFINITIONS, Definition, Group, LocalSettings, ServerSettings};
use crate::state::Pair;

/// An Accounts action. The Library window does it.
#[derive(Clone, Debug, PartialEq)]
pub enum AccountAction {
    SignOut(Pair),
    Remove(Pair),
    AddServer,
}

/// The choice `step` places away from the `current` value: the next one for 1, the previous one for -1. It wraps
/// at the ends. A value that is not in the list goes to the first choice.
pub fn step(options: &[(Value, String)], current: &Value, step: isize) -> Value {
    let len = options.len() as isize;
    let next = match options.iter().position(|(value, _)| value == current) {
        Some(index) => (index as isize + step).rem_euclid(len),
        None => 0,
    };
    options[next as usize].0.clone()
}

/// A Local setting on the Settings page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Local {
    HardwareDecoding,
    CheckReleases,
}

/// A row of the Settings page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    Server(&'static Definition),
    Local(Local),
    /// The index of the Server and Account pair.
    Account(usize),
    AddServer,
}

/// The visible sections of the page in order, each with its title and its rows. The Server settings show only
/// after the page has read them. A group with no visible rows does not show.
pub fn sections(server: bool, pairs: usize) -> Vec<(&'static str, Vec<Row>)> {
    let mut sections: Vec<_> = Group::ALL
        .into_iter()
        .map(|group| {
            let mut rows: Vec<_> =
                DEFINITIONS.iter().filter(|definition| server && definition.group == group).map(Row::Server).collect();
            if group == Group::ThisComputer {
                rows.extend([Row::Local(Local::HardwareDecoding), Row::Local(Local::CheckReleases)]);
            }
            (group.title(), rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect();
    sections.push(("Accounts", (0..pairs).map(Row::Account).chain([Row::AddServer]).collect()));
    sections
}

pub struct SettingsPage {
    client: Arc<Client>,
    pairs: Vec<Pair>,
    /// The Active server, which the Accounts section marks.
    active_pair: Option<(String, String)>,
    /// The Server settings, after the page read them.
    server: Option<ServerSettings>,
    /// The read failed. The page shows the error with Retry.
    load_error: Option<String>,
    /// The last failed write, or the last Accounts action.
    message: Option<(String, bool)>,
    local: LocalSettings,
    /// The release check of the app. `None` when the page has no Library window, as in a test.
    release: Option<Entity<ReleaseCheck>>,
    _release: Option<Subscription>,
    /// The index of the focused row in the list of visible rows.
    row: usize,
    /// On an Account row: 0 is Sign out, 1 is Remove.
    action: usize,
    /// The open Choice list below the focused row.
    picker: Option<ChoiceList>,
    /// The page has the keyboard focus.
    pub active: bool,
    fresh: Loader,
    /// The write that runs for each key. A new change of the key drops the old write.
    writes: HashMap<&'static str, Task<()>>,
}

impl EventEmitter<AccountAction> for SettingsPage {}

impl Reload for SettingsPage {
    fn deps(&self) -> Vec<Deps> {
        vec![Deps::Recovery]
    }

    fn fresh(&mut self, _: usize) -> &mut Fresh {
        &mut self.fresh
    }

    fn reload(&mut self, _: usize, cx: &mut Context<Self>) {
        self.load(cx);
    }
}

impl SettingsPage {
    pub fn new(
        client: Arc<Client>,
        pairs: Vec<Pair>,
        active_pair: Option<(String, String)>,
        release: Option<Entity<ReleaseCheck>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _release = release.as_ref().map(|release| cx.observe(release, |_, _, cx| cx.notify()));
        let mut page = Self {
            client,
            pairs,
            active_pair,
            server: None,
            load_error: None,
            message: None,
            local: LocalSettings::load(&crate::settings::directory(cx)),
            release,
            _release,
            row: 0,
            action: 0,
            picker: None,
            active: true,
            fresh: Loader::default(),
            writes: HashMap::new(),
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.load_error = None;
        let client = self.client.clone();
        Loader::run(
            self,
            cx,
            |this| &mut this.fresh,
            Failure::Retain,
            async move { client.load_server_settings().await.map(|()| client.server_settings()) },
            move |this, result, _cx| match result {
                Ok(settings) => {
                    // The Server rows can appear above the focused row. The focus stays on its row, and a focus
                    // on the first row stays at the top.
                    let focused = this.rows().get(this.row).copied().filter(|_| this.row > 0);
                    this.server = Some(settings);
                    if let Some(focused) = focused {
                        this.row = this.rows().iter().position(|row| *row == focused).unwrap_or(0);
                    }
                }
                Err(err) => this.load_error = Some(err.to_string()),
            },
            Self::load,
        );
        cx.notify();
    }

    /// The page shows again. A newer Settings page can have changed the Local settings, so the page reads them
    /// again, and a toggle here does not write back an old value.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        self.local = LocalSettings::load(&crate::settings::directory(cx));
        cx.notify();
    }

    /// The visible rows, in order.
    fn rows(&self) -> Vec<Row> {
        sections(self.server.is_some(), self.pairs.len()).into_iter().flat_map(|(_, rows)| rows).collect()
    }

    /// The Server setting of the focused row.
    fn focused_setting(&self) -> Option<&'static Definition> {
        match self.rows().get(self.row) {
            Some(Row::Server(definition)) => Some(definition),
            _ => None,
        }
    }

    /// The choices of a Server setting. They are empty before the page read the settings.
    fn options(&self, definition: &Definition) -> Vec<(Value, String)> {
        let Some(server) = &self.server else {
            return Vec::new();
        };
        definition.options(&server.value(definition.key))
    }

    /// Changes a Server setting by `delta` choices and writes it.
    fn change(&mut self, definition: &'static Definition, delta: isize, cx: &mut Context<Self>) {
        if let Some(server) = &self.server {
            let old = server.value(definition.key);
            self.set(definition, step(&self.options(definition), &old, delta), cx);
        }
    }

    /// The choices of a Server setting as the rows of a Choice list.
    fn choices(&self, definition: &Definition) -> Vec<Choice> {
        let Some(server) = &self.server else {
            return Vec::new();
        };
        let current = server.value(definition.key);
        self.options(definition).into_iter().map(|(value, label)| Choice::new(label, value == current)).collect()
    }

    /// Enter on a Server setting: it opens the Choice list, or toggles an On/Off setting.
    fn open_picker(&mut self, definition: &'static Definition, cx: &mut Context<Self>) {
        let choices = self.choices(definition);
        if choices.len() <= 2 {
            return self.change(definition, 1, cx);
        }
        self.picker = Some(ChoiceList::open(&choices));
    }

    /// Picks the choice at `focus` in the open list, and closes the list.
    fn pick(&mut self, focus: usize, cx: &mut Context<Self>) {
        self.picker = None;
        if let Some(definition) = self.focused_setting()
            && let Some((value, _)) = self.options(definition).into_iter().nth(focus)
        {
            self.set(definition, value, cx);
        }
        cx.notify();
    }

    /// Sets a Server setting and writes it.
    fn set(&mut self, definition: &'static Definition, new: Value, cx: &mut Context<Self>) {
        let Some(server) = &mut self.server else {
            return;
        };
        let key = definition.key;
        let old = server.value(key);
        server.set(key, &new);
        self.message = None;
        let client = self.client.clone();
        let write = cx.spawn(async move |this, cx| {
            let result = client.set_server_setting(key, new).await;
            this.update(cx, |this, cx| {
                // The task ends here, so it must not drop itself.
                if let Some(write) = this.writes.remove(key) {
                    write.detach();
                }
                if this.fresh.written() {
                    this.load(cx);
                }
                if let Err(err) = result {
                    if let Some(server) = &mut this.server {
                        server.set(key, &old);
                    }
                    this.message = Some((format!("Could not save {}: {err}", definition.label), true));
                }
                cx.notify();
            })
            .ok();
        });
        self.fresh.write();
        if self.writes.insert(key, write).is_some() {
            // The old write of the key is dropped. The new one still holds the reloads.
            self.fresh.written();
        }
        cx.notify();
    }

    /// Saves the Local settings. Returns false when the save failed, and shows why.
    fn save_local(&mut self, new: LocalSettings, label: &str, cx: &mut Context<Self>) -> bool {
        cx.notify();
        match new.save(&crate::settings::directory(cx)) {
            Ok(()) => {
                self.local = new;
                self.message = None;
                true
            }
            Err(err) => {
                self.message = Some((format!("Could not save {label}: {err}"), true));
                false
            }
        }
    }

    fn toggle_hwdec(&mut self, cx: &mut Context<Self>) {
        let new = LocalSettings { hardware_decoding: !self.local.hardware_decoding, ..self.local.clone() };
        self.save_local(new, "hardware decoding", cx);
    }

    /// Turns the release check on or off. On, the app checks at once.
    fn toggle_releases(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.local.check_releases;
        if self.save_local(LocalSettings { check_releases: enabled, ..self.local.clone() }, "the release check", cx)
            && let Some(release) = &self.release
        {
            release.update(cx, |release, cx| release.set_enabled(enabled, cx));
        }
    }

    fn act(&mut self, index: usize, action: usize, cx: &mut Context<Self>) {
        let Some(pair) = self.pairs.get(index).cloned() else {
            return;
        };
        let who = format!("{} on {}", pair.username, pair.server_name);
        if action == 0 {
            self.message = Some((format!("Signed out {who}."), false));
            cx.emit(AccountAction::SignOut(pair));
        } else {
            self.message = Some((format!("Removed {who}."), false));
            self.pairs.remove(index);
            self.row = self.row.min(self.rows().len() - 1);
            cx.emit(AccountAction::Remove(pair));
        }
        cx.notify();
    }

    /// A key while the page has the focus. Returns false for a key that the page does not use.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        if self.picker.is_some() {
            let len = self.focused_setting().map_or(0, |definition| self.options(definition).len());
            let outcome = self.picker.as_mut().map_or(Outcome::Unused, |picker| picker.key(key, len));
            match outcome {
                Outcome::Moved => {}
                Outcome::Picked(index) => self.pick(index, cx),
                Outcome::Closed => self.picker = None,
                Outcome::Unused => return false,
            }
            cx.notify();
            return true;
        }
        let rows = self.rows();
        // The list can be shorter than when the row had the focus, for example after a failed read.
        self.row = self.row.min(rows.len() - 1);
        match (key, rows[self.row]) {
            (Key::Down, _) => {
                self.row = (self.row + 1).min(rows.len() - 1);
                self.action = 0;
            }
            (Key::Up, _) => {
                self.row = self.row.saturating_sub(1);
                self.action = 0;
            }
            (Key::Other("r"), _) if self.load_error.is_some() => self.load(cx),
            (Key::Right, Row::Server(definition)) => self.change(definition, 1, cx),
            (Key::Other("enter"), Row::Server(definition)) => self.open_picker(definition, cx),
            (Key::Left, Row::Server(definition)) => self.change(definition, -1, cx),
            (Key::Right | Key::Other("enter") | Key::Left, Row::Local(Local::HardwareDecoding)) => {
                self.toggle_hwdec(cx)
            }
            (Key::Right | Key::Other("enter") | Key::Left, Row::Local(Local::CheckReleases)) => {
                self.toggle_releases(cx)
            }
            (Key::Other("enter"), Row::Account(index)) => self.act(index, self.action, cx),
            (Key::Right, Row::Account(_)) => self.action = 1,
            (Key::Left, Row::Account(_)) if self.action == 1 => self.action = 0,
            (Key::Other("enter"), Row::AddServer) => cx.emit(AccountAction::AddServer),
            _ => return false,
        }
        cx.notify();
        true
    }

    fn header(&self, text: &'static str, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        div()
            .pt(px(24.))
            .pb(px(6.))
            .text_size(px(13.))
            .font_weight(FontWeight::BOLD)
            .text_color(t.secondary)
            .child(text)
    }

    /// One row: a label on the left and a value on the right. A click acts like Enter. A click on the row of
    /// the open list closes the list.
    fn row(&self, row: usize, label: SharedString, value: SharedString, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let focused = self.active && self.row == row;
        div()
            .id(("settings-row", row))
            .clickable()
            .max_w(px(720.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .flex()
            .items_center()
            .gap(px(14.))
            .bg(t.surface)
            .border_2()
            .border_color(if focused { t.accent } else { gpui::transparent_black() })
            .on_click(cx.listener(move |this, _, _, cx| {
                let close = this.picker.take().is_some() && this.row == row;
                this.row = row;
                if !close {
                    this.key(Key::Other("enter"), cx);
                }
                cx.notify();
            }))
            .child(div().flex_1().truncate().child(label))
            .child(div().text_color(t.bright).child(value))
    }

    /// The open Choice list below a row, with a maximum height. A click on a choice picks it.
    fn picker(&self, picker: &ChoiceList, choices: Vec<Choice>, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let this = cx.entity().downgrade();
        let on_pick = move |i, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.pick(i, cx)).ok();
        };
        picker
            .render("settings-choice", choices, on_pick, cx)
            .max_w(px(720.))
            .max_h(px(320.))
            .p(px(6.))
            .rounded(px(10.))
            .bg(t.surface)
    }

    fn pair_row(&self, row: usize, index: usize, pair: &Pair, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let is_active = self.active_pair == Some((pair.server_id.clone(), pair.account_id.clone()));
        let action = |n: usize, label: &'static str| {
            let focused = self.active && self.row == row && self.action == n;
            div()
                .id(("settings-action", row * 2 + n))
                .clickable()
                .px(px(12.))
                .py(px(4.))
                .rounded_full()
                .border_2()
                .border_color(if focused { t.accent } else { t.divider() })
                .text_color(if n == 1 { t.danger } else { t.foreground })
                .on_click(cx.listener(move |this, _, _, cx| {
                    // The open Choice list belongs to the old row. It must close before the row changes.
                    this.picker = None;
                    this.row = row;
                    this.action = n;
                    this.act(index, n, cx);
                }))
                .child(label)
        };
        div()
            .max_w(px(720.))
            .px(px(14.))
            .py(px(8.))
            .rounded(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(t.surface)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_color(t.bright).child(pair.username.clone()))
                    .child(div().truncate().text_size(px(12.)).text_color(t.secondary).child(pair.server_name.clone())),
            )
            .when(is_active, |row| row.child(div().text_size(px(12.)).text_color(t.accent).child("Active")))
            .child(action(0, "Sign out"))
            .child(action(1, "Remove"))
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let on_off = |on: bool| if on { "On" } else { "Off" };
        let mut index = 0;
        let mut elements = Vec::new();
        for (title, rows) in sections(self.server.is_some(), self.pairs.len()) {
            elements.push(self.header(title, cx).into_any_element());
            for row in rows {
                elements.push(match row {
                    Row::Server(definition) => {
                        let current = self.server.as_ref().map(|server| server.value(definition.key));
                        let value = self
                            .options(definition)
                            .into_iter()
                            .find(|(v, _)| Some(v) == current.as_ref())
                            .map(|(_, l)| l)
                            .unwrap_or_default();
                        self.row(index, definition.label.into(), value.into(), cx).into_any_element()
                    }
                    Row::Local(local) => {
                        let (label, on) = match local {
                            Local::HardwareDecoding => ("Hardware decoding", self.local.hardware_decoding),
                            Local::CheckReleases => ("Check for new releases", self.local.check_releases),
                        };
                        self.row(index, label.into(), on_off(on).into(), cx).into_any_element()
                    }
                    Row::Account(i) => self.pair_row(index, i, &self.pairs[i], cx).into_any_element(),
                    Row::AddServer => self.row(index, "Add server".into(), "".into(), cx).into_any_element(),
                });
                if let (Row::Server(definition), Some(picker)) =
                    (row, self.picker.as_ref().filter(|_| self.row == index))
                {
                    elements.push(self.picker(picker, self.choices(definition), cx).into_any_element());
                }
                index += 1;
            }
        }
        let status = self.server.is_none().then(|| {
            let retry = button("settings-retry", "Retry", ButtonVariant::Secondary, &*cx)
                .debug_selector(|| "settings-retry".into())
                .on_click(cx.listener(|this, _, _, cx| this.load(cx)));
            let failed = self.load_error.is_some();
            div()
                .pt(px(24.))
                .flex()
                .items_center()
                .gap(px(14.))
                .text_color(if failed { t.danger } else { t.secondary })
                .child(self.load_error.clone().unwrap_or_else(|| "Loading…".into()))
                .when(failed, |row| row.child(retry))
        });
        let newer = self.release.as_ref().and_then(|release| release.read(cx).release.clone());
        div()
            .id("settings-page")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(40.))
            .pt(px(32.))
            .pb(px(40.))
            // ponytail: the page does not scroll to the focused row; it fits a normal window.
            .overflow_y_scroll()
            .child(div().text_size(px(28.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("Settings"))
            .children(status)
            .children(elements)
            .children(self.message.clone().map(|(text, failed)| {
                div().pt(px(12.)).text_color(if failed { t.danger } else { t.secondary }).child(text)
            }))
            .child(div().flex_1())
            .child(
                div()
                    .pt(px(24.))
                    .flex()
                    .gap(px(14.))
                    .text_size(px(12.))
                    .text_color(t.secondary)
                    .child(crate::BUILD_IDENTITY)
                    .children(newer.map(|release| {
                        div()
                            .id("settings-release")
                            .clickable()
                            .text_color(t.accent)
                            .on_click(move |_, _, cx| cx.open_url(&release.url))
                            .child(format!("Relics {} is out", release.version))
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Local, Row, sections, step};

    #[test]
    fn a_step_wraps_at_the_ends_and_an_unknown_value_goes_to_the_first_choice() {
        let options =
            [(json!("auto"), "Auto".to_string()), (json!("always"), String::new()), (json!("off"), String::new())];
        assert_eq!(step(&options, &json!("auto"), 1), json!("always"));
        assert_eq!(step(&options, &json!("off"), 1), json!("auto"));
        assert_eq!(step(&options, &json!("auto"), -1), json!("off"));
        assert_eq!(step(&options, &Value::Null, 1), json!("auto"));
    }

    #[test]
    fn the_server_settings_show_in_the_groups_of_the_web_and_empty_groups_do_not_show() {
        let shown: Vec<_> = sections(true, 1)
            .into_iter()
            .map(|(title, rows)| {
                let rows: Vec<_> = rows
                    .into_iter()
                    .map(|row| match row {
                        Row::Server(definition) => definition.label.to_string(),
                        row => format!("{row:?}"),
                    })
                    .collect();
                (title, rows)
            })
            .collect();
        let expected = [
            ("Skipping", vec!["Skip intros", "Auto-skip recaps", "Auto-skip credits"]),
            ("Next episode", vec!["Auto-play next episode"]),
            (
                "Language & subtitles",
                vec!["Spoken language", "Subtitle language", "Subtitle behavior", "Show forced subtitles"],
            ),
            ("This computer", vec!["Local(HardwareDecoding)", "Local(CheckReleases)"]),
            ("Accounts", vec!["Account(0)", "AddServer"]),
        ];
        let expected: Vec<_> =
            expected.map(|(title, rows)| (title, rows.into_iter().map(String::from).collect::<Vec<_>>())).into();
        assert_eq!(shown, expected);
    }

    #[test]
    fn before_the_read_only_the_local_settings_and_the_accounts_show() {
        assert_eq!(
            sections(false, 0),
            [
                ("This computer", vec![Row::Local(Local::HardwareDecoding), Row::Local(Local::CheckReleases)]),
                ("Accounts", vec![Row::AddServer]),
            ]
        );
    }
}
