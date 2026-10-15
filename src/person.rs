//! The Person page: the photo, the name, the birth and death dates with the age, the birthplace and the
//! biography, over a filmography grid with the tabs All, Movies and Series. Up from the top row of the grid
//! goes to the tabs, and Down from the tabs goes back to the grid.

use std::sync::Arc;

use chrono::NaiveDate;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement, Render, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::artwork;
use crate::click::Clickable as _;
use crate::grid::{Grid, Move};
use crate::home::{Open, Play};
use crate::keys::Key;
use crate::pages::freshness::{Deps, Fresh, Reload};
use crate::search::{OpenPerson, person_query};

const PHOTO_WIDTH: f32 = 180.;

/// The filmography tabs: the label and the media type of the query.
const TABS: [(&str, Option<&str>); 3] = [("All", None), ("Movies", Some("movie")), ("Series", Some("series"))];

/// The lines about the life of a Person: "Born Jun 11, 1933", then "Died Aug 29, 2016 (age 83)", or the age
/// on `today` ("92 years old") while the Person lives. A date that does not parse gives no line.
pub fn life(birth: Option<&str>, death: Option<&str>, today: NaiveDate) -> Vec<String> {
    let parse = |date: Option<&str>| date.and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
    let (birth, death) = (parse(birth), parse(death));
    let day = |date: NaiveDate| date.format("%b %-d, %Y").to_string();
    let age = |end: NaiveDate| birth.and_then(|birth| end.years_since(birth));
    let mut lines: Vec<String> = birth.map(|birth| format!("Born {}", day(birth))).into_iter().collect();
    match death {
        Some(death) => lines.push(match age(death) {
            Some(age) => format!("Died {} (age {age})", day(death)),
            None => format!("Died {}", day(death)),
        }),
        None => lines.extend(age(today).map(|age| format!("{age} years old"))),
    }
    lines
}

pub struct PersonPage {
    client: Arc<Client>,
    id: String,
    name: String,
    /// The Person, after the call returns.
    person: Option<wire::Person>,
    /// The error of the Person call, shown with Retry. The filmography loads without the Person.
    error: Option<String>,
    /// The open tab of `TABS`.
    tab: usize,
    pub grid: Entity<Grid>,
    /// The tabs have the keyboard focus, not the grid.
    in_tabs: bool,
    /// The focused tab, while the tabs have the keyboard focus.
    tab_focus: usize,
    /// The page has the keyboard focus.
    active: bool,
    fresh: Fresh,
    _load: Task<()>,
    _events: Vec<Subscription>,
}

impl Reload for PersonPage {
    fn deps(&self) -> Vec<Deps> {
        vec![Deps::Catalog(None)]
    }

    fn fresh(&mut self, _: usize) -> &mut Fresh {
        &mut self.fresh
    }

    fn reload(&mut self, _: usize, cx: &mut Context<Self>) {
        self.load(cx);
    }
}

impl EventEmitter<Play> for PersonPage {}
impl EventEmitter<Open> for PersonPage {}

impl PersonPage {
    pub fn new(client: Arc<Client>, person: &OpenPerson, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let grid = cx.new(|cx| Grid::new(client.clone(), person_query(&person.id, None), window, cx));
        let events = vec![
            cx.subscribe(&grid, |_, _, Play(item), cx| cx.emit(Play(item.clone()))),
            cx.subscribe(&grid, |_, _, Open(item), cx| cx.emit(Open(item.clone()))),
        ];
        let mut page = Self {
            client,
            id: person.id.clone(),
            name: person.name.clone(),
            person: None,
            error: None,
            tab: 0,
            grid,
            in_tabs: false,
            tab_focus: 0,
            active: false,
            fresh: Fresh::default(),
            _load: Task::ready(()),
            _events: events,
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let (client, id) = (self.client.clone(), self.id.clone());
        let ticket = self.fresh.begin();
        self._load = cx.spawn(async move |this, cx| {
            let result = client.person(&id).await;
            this.update(cx, |this, cx| {
                match result {
                    _ if !this.fresh.end(ticket) => {}
                    Ok(person) => this.person = Some(person),
                    // The page keeps the Person that it shows, with no error.
                    Err(err) if this.person.is_some() => eprintln!("relics: cannot reload person {}: {err}", this.id),
                    // The Library window shows Offline or the sign-in screen. The page shows Retry.
                    Err(err) => this.error = Some(err.to_string()),
                }
                if this.fresh.ready() {
                    this.load(cx);
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        self.sync(cx);
    }

    /// Gives the focus ring of the grid to the grid while the grid has the keyboard focus.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let content = self.active && !self.in_tabs;
        self.grid.update(cx, |grid, cx| {
            grid.active = content;
            cx.notify();
        });
        cx.notify();
    }

    fn open_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.in_tabs = true;
        self.tab_focus = tab;
        if tab != self.tab {
            self.tab = tab;
            let query = person_query(&self.id, TABS[tab].1);
            self.grid.update(cx, |grid, cx| grid.set_query(query, cx));
        }
        self.sync(cx);
    }

    /// Handles a key. Returns false when the page does not use it, for example h or Left at the left edge.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        if self.in_tabs {
            match key {
                Key::Left if self.tab_focus == 0 => return false,
                Key::Left => self.tab_focus -= 1,
                Key::Right => self.tab_focus = (self.tab_focus + 1).min(TABS.len() - 1),
                Key::Down => {
                    self.in_tabs = false;
                    self.sync(cx);
                }
                Key::Other("enter") => self.open_tab(self.tab_focus, cx),
                _ => return false,
            }
            cx.notify();
            return true;
        }
        match self.grid.update(cx, |grid, cx| grid.key(key, cx)) {
            Move::To(_) => true,
            Move::OutTop => {
                self.in_tabs = true;
                self.tab_focus = self.tab;
                self.sync(cx);
                true
            }
            Move::OutLeft | Move::Stay => false,
        }
    }

    /// A mouse-down on the tabs or on the grid gives that part the keyboard focus.
    fn focus_part(&mut self, in_tabs: bool, cx: &mut Context<Self>) {
        if in_tabs && !self.in_tabs {
            self.tab_focus = self.tab;
        }
        self.in_tabs = in_tabs;
        self.sync(cx);
    }

    fn header(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let person = self.person.as_ref();
        let url = person
            .and_then(|p| p.photo_url.as_deref())
            .filter(|url| !url.is_empty())
            .and_then(|url| self.client.server().url.join(url).ok());
        let today = chrono::Local::now().date_naive();
        let lines = person.map(|p| life(p.birth_date.as_deref(), p.death_date.as_deref(), today)).unwrap_or_default();
        let badges = lines.into_iter().map(|line| {
            div().px(px(10.)).py(px(4.)).rounded(px(6.)).bg(t.surface).text_size(px(12.)).child(line.to_uppercase())
        });
        let birthplace = person.and_then(|p| p.birthplace.clone()).filter(|place| !place.is_empty());
        let bio = person.and_then(|p| p.bio.clone()).filter(|bio| !bio.is_empty());
        let error = self.error.clone().map(|error| {
            div().flex().items_center().gap(px(14.)).text_color(t.danger).child(error).child(
                button("person-retry", "Retry", ButtonVariant::Secondary, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            )
        });
        div()
            .px(px(40.))
            .pt(px(28.))
            .flex()
            .gap(px(28.))
            .child(
                div()
                    .w(px(PHOTO_WIDTH))
                    .h(px(PHOTO_WIDTH * 1.5))
                    .flex_none()
                    .rounded(px(12.))
                    .overflow_hidden()
                    .bg(t.surface)
                    .child(artwork::image(url.map(String::from), None, PHOTO_WIDTH)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(34.))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(t.bright)
                            .child(self.name.clone()),
                    )
                    .child(div().flex().flex_wrap().gap(px(8.)).text_color(t.secondary).children(badges))
                    .children(birthplace.map(|place| div().text_color(t.secondary).child(place)))
                    // A long biography scrolls in its own area, so that the filmography stays on the page.
                    .children(bio.map(|bio| {
                        div().id("person-bio").max_h(px(150.)).overflow_y_scroll().line_height(px(21.)).child(bio)
                    }))
                    .children(error),
            )
    }

    fn tabs(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let tabs = TABS.iter().enumerate().map(|(i, &(label, _))| {
            let (on, focused) = (i == self.tab, self.active && self.in_tabs && self.tab_focus == i);
            div()
                .id(("person-tab", i))
                .clickable()
                .px(px(18.))
                .py(px(9.))
                .rounded_full()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .bg(if on { t.foreground } else { gpui::hsla(0., 0., 1., 0.1) })
                .text_color(if on { t.background } else { t.foreground })
                .border_2()
                .border_color(if focused { t.accent } else { gpui::transparent_black() })
                .on_click(cx.listener(move |this, _, _, cx| this.open_tab(i, cx)))
                .child(label)
        });
        div()
            .px(px(40.))
            .pt(px(24.))
            .pb(px(18.))
            .flex()
            .items_center()
            .gap(px(8.))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.focus_part(true, cx)))
            .child(div().pr(px(8.)).text_size(px(20.)).font_weight(FontWeight::BOLD).child("Filmography"))
            .children(tabs)
    }
}

impl Render for PersonPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let content = if self.grid.read(cx).is_empty() {
            div().px(px(40.)).text_color(t.secondary).child("No titles.").into_any_element()
        } else {
            div().size_full().px(px(40.)).child(self.grid.clone()).into_any_element()
        };
        div().size_full().flex().flex_col().child(self.header(cx)).child(self.tabs(cx)).child(
            div()
                .flex_1()
                .min_h_0()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.focus_part(false, cx)))
                .child(content),
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::life;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
    }

    #[test]
    fn a_living_person_shows_the_birth_date_and_the_age_today() {
        assert_eq!(life(Some("1940-04-25"), None, today()), ["Born Apr 25, 1940", "85 years old"]);
        assert_eq!(life(Some("1940-02-28"), None, today()), ["Born Feb 28, 1940", "86 years old"]);
    }

    #[test]
    fn a_dead_person_shows_the_age_at_death() {
        assert_eq!(
            life(Some("1933-06-11"), Some("2016-08-29"), today()),
            ["Born Jun 11, 1933", "Died Aug 29, 2016 (age 83)"]
        );
    }

    #[test]
    fn a_missing_or_bad_date_gives_no_line_and_no_age() {
        assert_eq!(life(None, Some("2016-08-29"), today()), ["Died Aug 29, 2016"]);
        assert_eq!(life(Some("1933"), None, today()), Vec::<String>::new());
        assert_eq!(life(None, None, today()), Vec::<String>::new());
    }
}
