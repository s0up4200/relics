//! The Search page and the Person grid. Search sends the text 200 ms after the last key. It shows the matching
//! people in a row above a grid of the matching titles. Enter on a person opens the Person grid: the shared poster
//! grid of the titles that the Person took part in.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement,
    Render, RenderImage, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::artwork::{self, ArtworkCache};
use crate::browse::PAGE_SIZE;
use crate::grid::{Grid, Move};
use crate::home::{Open, Play};
use crate::keys::Key;

/// The pause after the last key before Search sends the text.
pub const DEBOUNCE: Duration = Duration::from_millis(200);
const PHOTO: f32 = 96.;

/// The user pressed Enter on a person in Search or on a cast card of the detail page.
pub struct OpenPerson {
    pub id: String,
    pub name: String,
}

/// The query of the titles that match `text`.
pub fn query(text: &str) -> wire::CatalogQuery {
    wire::CatalogQuery {
        source: Some("query".into()),
        q: Some(text.into()),
        type_: Some("video".into()),
        limit: std::num::NonZeroU64::new(PAGE_SIZE),
        ..Default::default()
    }
}

/// The query of the Person grid: the titles of the Person, the newest first, as in the web client.
pub fn person_query(person_id: &str) -> wire::CatalogQuery {
    wire::CatalogQuery {
        source: Some("person".into()),
        person_id: person_id.try_into().ok(),
        sort: Some("year".into()),
        order: Some("desc".into()),
        limit: std::num::NonZeroU64::new(PAGE_SIZE),
        ..Default::default()
    }
}

/// What a key did in the search field.
#[derive(Debug, PartialEq, Eq)]
pub enum FieldKey {
    /// The text changed. The search is due after `DEBOUNCE`.
    Edited,
    /// The first Esc removed the text.
    Cleared,
    /// Down or Enter: the focus goes into the results.
    Results,
    /// Esc in an empty field: the page goes back.
    Back,
    /// The field used the key, but nothing changed.
    Stay,
    /// The field does not use the key.
    Unused,
}

/// The text of the search field and the time when it is due to be sent. The caller gives the time, so a test
/// checks the pause without a clock.
#[derive(Debug, Default)]
pub struct Field {
    text: String,
    due: Option<Instant>,
}

impl Field {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Handles a key at `now`. `typed` is the text that the key types, if any, so j types a letter and the Down
    /// arrow moves into the results.
    pub fn key(&mut self, key: Key, typed: Option<&str>, now: Instant) -> FieldKey {
        match key {
            Key::Other("escape") if self.text.is_empty() => FieldKey::Back,
            Key::Other("escape") => {
                self.text.clear();
                self.due = None;
                FieldKey::Cleared
            }
            Key::Other("enter") => FieldKey::Results,
            Key::Other("backspace") if self.text.pop().is_none() => FieldKey::Stay,
            Key::Other("backspace") => {
                self.due = Some(now + DEBOUNCE);
                FieldKey::Edited
            }
            _ => match typed.filter(|text| !text.is_empty() && !text.chars().any(char::is_control)) {
                Some(text) => {
                    self.text.push_str(text);
                    self.due = Some(now + DEBOUNCE);
                    FieldKey::Edited
                }
                None if key == Key::Down => FieldKey::Results,
                None => FieldKey::Unused,
            },
        }
    }

    /// The text to send, once at `now` the pause after the last edit is over. `None` before that.
    pub fn take_due(&mut self, now: Instant) -> Option<String> {
        self.due.filter(|due| now >= *due)?;
        self.due = None;
        Some(self.text.trim().to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Field,
    Person(usize),
    Titles,
}

pub struct Search {
    client: Arc<Client>,
    field: Field,
    focus: Focus,
    /// The page has the keyboard focus, so the focused control shows the focus ring.
    pub active: bool,
    /// The text of the results that show. Empty: no results show.
    shown: String,
    people: Vec<wire::Person>,
    people_thumbs: Vec<Option<Arc<RenderImage>>>,
    people_error: Option<String>,
    people_loading: bool,
    /// The grid of the titles, made at the first search.
    titles: Option<Entity<Grid>>,
    artwork: Entity<ArtworkCache>,
    _debounce: Task<()>,
    _people: Task<()>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for Search {}
impl EventEmitter<Open> for Search {}
impl EventEmitter<OpenPerson> for Search {}

impl Search {
    pub fn new(client: Arc<Client>, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.drop_thumbs(cx)).detach();
        Self {
            client,
            field: Field::default(),
            focus: Focus::Field,
            active: false,
            shown: String::new(),
            people: Vec::new(),
            people_thumbs: Vec::new(),
            people_error: None,
            people_loading: false,
            titles: None,
            artwork: artwork::global(cx),
            _debounce: Task::ready(()),
            _people: Task::ready(()),
            _events: Vec::new(),
        }
    }

    fn drop_thumbs(&mut self, cx: &mut gpui::App) {
        for thumb in self.people_thumbs.drain(..).flatten() {
            cx.drop_image(thumb, None);
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        self.set_focus(self.focus, cx);
    }

    /// Puts the keyboard focus in the search field, for example when the user presses /.
    pub fn focus_field(&mut self, cx: &mut Context<Self>) {
        self.set_focus(Focus::Field, cx);
    }

    fn set_focus(&mut self, focus: Focus, cx: &mut Context<Self>) {
        self.focus = focus;
        let titles = self.active && focus == Focus::Titles;
        if let Some(grid) = &self.titles {
            grid.update(cx, |grid, cx| {
                grid.active = titles;
                cx.notify();
            });
        }
        cx.notify();
    }

    fn has_titles(&self, cx: &gpui::App) -> bool {
        !self.shown.is_empty() && self.titles.as_ref().is_some_and(|grid| !grid.read(cx).is_empty())
    }

    /// Sends `text` for the titles and the people. A new search drops the requests of the old one.
    fn search(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_thumbs(cx);
        self.people.clear();
        self.people_error = None;
        self._people = Task::ready(());
        self.shown = text.clone();
        self.set_focus(Focus::Field, cx);
        if text.is_empty() {
            self.people_loading = false;
            return;
        }
        let query = query(&text);
        match &self.titles {
            Some(grid) => grid.update(cx, |grid, cx| grid.set_query(query, cx)),
            None => {
                let client = self.client.clone();
                let grid = cx.new(|cx| Grid::new(client, query, window, cx));
                self._events.push(cx.subscribe(&grid, |_, _, Play(item), cx| cx.emit(Play(item.clone()))));
                self._events.push(cx.subscribe(&grid, |_, _, Open(item), cx| cx.emit(Open(item.clone()))));
                self.titles = Some(grid);
            }
        }
        self.people_loading = true;
        let client = self.client.clone();
        self._people = cx.spawn(async move |this, cx| {
            let result = client.people(&text).await;
            this.update(cx, |this, cx| {
                this.people_loading = false;
                match result {
                    Ok(people) => {
                        this.people_thumbs =
                            people.iter().map(|p| p.photo_thumbhash.as_deref().and_then(artwork::thumbhash)).collect();
                        this.people = people;
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                    }
                    // The Library window shows Offline or the sign-in screen. The row shows Retry.
                    Err(err) => this.people_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn open_person(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(person) = self.people.get(index) {
            self.focus = Focus::Person(index);
            cx.emit(OpenPerson { id: person.id.to_string(), name: person.name.clone() });
        }
    }

    /// Handles a key. `typed` is the text that the key types. Returns false when the page does not use the key,
    /// for example Esc in an empty field.
    pub fn key(&mut self, key: Key, typed: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let focus = match self.focus {
            Focus::Field => match self.field.key(key, typed, Instant::now()) {
                FieldKey::Edited => {
                    // A new key drops the timer of the old one.
                    self._debounce = cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(DEBOUNCE).await;
                        this.update_in(cx, |this, window, cx| {
                            if let Some(text) = this.field.take_due(Instant::now()) {
                                this.search(text, window, cx);
                            }
                        })
                        .ok();
                    });
                    Focus::Field
                }
                FieldKey::Cleared => {
                    self._debounce = Task::ready(());
                    self.search(String::new(), window, cx);
                    Focus::Field
                }
                FieldKey::Results if !self.people.is_empty() => Focus::Person(0),
                FieldKey::Results if self.has_titles(cx) => Focus::Titles,
                FieldKey::Results | FieldKey::Stay => Focus::Field,
                FieldKey::Back | FieldKey::Unused => return false,
            },
            _ if key == Key::Other("/") => Focus::Field,
            Focus::Person(i) => match key {
                Key::Other("enter") => {
                    self.open_person(i, cx);
                    return true;
                }
                Key::Left if i > 0 => Focus::Person(i - 1),
                Key::Right if i + 1 < self.people.len() => Focus::Person(i + 1),
                Key::Right => Focus::Person(i),
                Key::Down if self.has_titles(cx) => Focus::Titles,
                Key::Up => Focus::Field,
                _ => return false,
            },
            Focus::Titles => {
                let Some(grid) = self.titles.clone() else {
                    return false;
                };
                match grid.update(cx, |grid, cx| grid.key(key, cx)) {
                    Move::To(_) => Focus::Titles,
                    Move::OutTop if !self.people.is_empty() => Focus::Person(0),
                    Move::OutTop => Focus::Field,
                    Move::OutLeft | Move::Stay => return false,
                }
            }
        };
        self.set_focus(focus, cx);
        true
    }

    fn people_row(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let cards = self.people.iter().enumerate().map(|(i, person)| {
            let focused = self.active && self.focus == Focus::Person(i);
            let url = person.photo_url.as_deref().and_then(|url| self.client.server().url.join(url).ok());
            div()
                .id(("search-person", i))
                .w(px(PHOTO + 16.))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .on_click(cx.listener(move |this, _, _, cx| this.open_person(i, cx)))
                .child(
                    div()
                        .size(px(PHOTO))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(t.surface)
                        .border_2()
                        .border_color(if focused { t.accent } else { gpui::transparent_black() })
                        .child(artwork::image(
                            url.map(String::from),
                            self.people_thumbs.get(i).cloned().flatten(),
                            PHOTO,
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(13.))
                        .text_center()
                        .text_color(if focused { t.bright } else { t.foreground })
                        .truncate()
                        .child(person.name.clone()),
                )
        });
        div().id("search-people").flex().gap(px(12.)).overflow_x_scroll().children(cards)
    }
}

impl Render for Search {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let in_field = self.active && self.focus == Focus::Field;
        let text = self.field.text();
        let field = div()
            .w_full()
            .max_w(px(640.))
            .px(px(16.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(t.surface)
            .border_2()
            .border_color(if in_field { t.accent } else { gpui::transparent_black() })
            .text_size(px(16.))
            .text_color(if text.is_empty() { t.secondary } else { t.bright })
            .child(match (text.is_empty(), in_field) {
                (true, _) => "Titles and people".to_string(),
                (false, true) => format!("{text}▏"),
                (false, false) => text.to_string(),
            });
        let mut header =
            div().px(px(40.)).pt(px(28.)).pb(px(18.)).flex().flex_col().gap(px(18.)).child(
                div().text_size(px(34.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("Search"),
            );
        header = header.child(field);
        if let Some(error) = &self.people_error {
            header =
                header.child(div().flex().items_center().gap(px(14.)).text_color(t.danger).child(error.clone()).child(
                    button("search-retry", "Retry", ButtonVariant::Secondary, cx).on_click(cx.listener(
                        |this, _, window, cx| {
                            let text = this.shown.clone();
                            this.search(text, window, cx);
                        },
                    )),
                ));
        }
        if !self.people.is_empty() {
            header = header.child(self.people_row(cx));
        }
        let titles = self.titles.as_ref().filter(|_| !self.shown.is_empty());
        let nothing = titles.is_some_and(|grid| grid.read(cx).is_empty())
            && self.people.is_empty()
            && !self.people_loading
            && self.people_error.is_none();
        let content = match titles {
            _ if nothing => div()
                .px(px(40.))
                .text_size(px(20.))
                .text_color(t.bright)
                .child(format!("No results for {}", self.shown))
                .into_any_element(),
            Some(grid) => div().size_full().px(px(40.)).child(grid.clone()).into_any_element(),
            None => div().into_any_element(),
        };
        div().size_full().flex().flex_col().child(header).child(div().flex_1().min_h_0().child(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_go_into_the_field_and_the_search_is_due_200_ms_after_the_last_key() {
        let start = Instant::now();
        let ms = |n| start + Duration::from_millis(n);
        let mut field = Field::default();
        assert_eq!(field.key(Key::parse("h"), Some("H"), ms(0)), FieldKey::Edited);
        assert_eq!(field.key(Key::parse("space"), Some(" "), ms(50)), FieldKey::Edited);
        assert_eq!(field.key(Key::parse("j"), Some("j"), ms(100)), FieldKey::Edited);
        assert_eq!(field.key(Key::parse("1"), Some("1"), ms(150)), FieldKey::Edited);
        assert_eq!(field.text(), "H j1");
        assert_eq!(field.take_due(ms(299)), None, "the pause starts again at each key");
        assert_eq!(field.take_due(ms(350)), Some("H j1".into()));
        assert_eq!(field.take_due(ms(900)), None, "a search is sent once");
        assert_eq!(field.key(Key::parse("backspace"), None, ms(1000)), FieldKey::Edited);
        assert_eq!(field.take_due(ms(1200)), Some("H j".into()));
    }

    #[test]
    fn the_first_esc_clears_the_field_and_the_second_goes_back() {
        let now = Instant::now();
        let mut field = Field::default();
        field.key(Key::parse("a"), Some("a"), now);
        assert_eq!(field.key(Key::parse("escape"), None, now), FieldKey::Cleared);
        assert_eq!(field.text(), "");
        assert_eq!(field.take_due(now + DEBOUNCE), None, "a cleared field sends nothing");
        assert_eq!(field.key(Key::parse("escape"), None, now), FieldKey::Back);
    }

    #[test]
    fn down_or_enter_moves_into_the_results_and_other_keys_stay_out_of_the_text() {
        let now = Instant::now();
        let mut field = Field::default();
        assert_eq!(field.key(Key::parse("down"), None, now), FieldKey::Results);
        assert_eq!(field.key(Key::parse("enter"), Some("\n"), now), FieldKey::Results);
        assert_eq!(
            field.key(Key::parse("backspace"), None, now),
            FieldKey::Stay,
            "Backspace in an empty field does not go back"
        );
        assert_eq!(field.key(Key::parse("left"), None, now), FieldKey::Unused);
        assert_eq!(field.key(Key::parse("tab"), Some("\t"), now), FieldKey::Unused);
        assert_eq!(field.text(), "");
    }
}
