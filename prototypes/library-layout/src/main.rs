//! PROTOTYPE for "Library window look and layout" (s0up4200/silo-omarchy#10).
//! Throwaway. Three layouts of the library window over fake data, switched with F1/F2/F3.
//!   A, Web clone: a sidebar, a hero, rows of cards and a full-page detail view, as in the Silo web client.
//!   B, Split pane: tabs, one list on the left and a detail pane on the right.
//!   C, Grid sheet: a filter bar and one poster grid. The detail view opens as a sheet over the grid.

mod data;

use data::{Catalog, Item, Kind, catalog};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Div, FocusHandle, FontWeight, KeyDownEvent, ScrollHandle,
    Window, WindowBounds, WindowOptions, div, hsla, linear_color_stop, linear_gradient, px, size,
};
use gpui_omarchy::{ActiveTheme, Theme, keycap};

const NAMES: [&str; 3] = ["Web clone", "Split pane", "Grid sheet"];

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    Library(usize),
    Detail(usize),
}

struct Proto {
    cat: Catalog,
    variant: usize,
    focus: FocusHandle,
    page: Page,
    back: Vec<(Page, usize, usize)>,
    /// Cursor: row (home section, or 0 = actions / 1 = episodes in detail) and column.
    row: usize,
    col: usize,
    /// Grid columns measured in the last render.
    cols: usize,
    /// Variant B: tab (0 home, 1 movies, 2 shows) and list selection.
    tab: usize,
    sel: usize,
    /// Variant C: sheet, filter text, typing mode, strip vs grid zone.
    sheet: Option<usize>,
    query: String,
    typing: bool,
    status: String,
    page_scroll: ScrollHandle,
    row_scrolls: Vec<ScrollHandle>,
    grid_scroll: ScrollHandle,
}

impl Proto {
    fn new(variant: usize, cx: &mut Context<Self>) -> Self {
        let cat = catalog();
        let row_scrolls = (0..8).map(|_| ScrollHandle::new()).collect();
        Self {
            cat,
            variant,
            focus: cx.focus_handle(),
            page: Page::Home,
            back: vec![],
            row: 0,
            col: 0,
            cols: 6,
            tab: 0,
            sel: 0,
            sheet: None,
            query: String::new(),
            typing: false,
            status: String::new(),
            page_scroll: ScrollHandle::new(),
            row_scrolls,
            grid_scroll: ScrollHandle::new(),
        }
    }

    fn item(&self, id: usize) -> &Item {
        &self.cat.items[id]
    }

    fn play(&mut self, id: usize) {
        let it = &self.cat.items[id];
        let verb = if it.progress.is_some() { "Resume" } else { "Play" };
        self.status = format!("{verb} “{}” in silo-player", it.title);
    }

    fn open(&mut self, page: Page) {
        self.back.push((self.page, self.row, self.col));
        self.page = page;
        self.row = 0;
        self.col = 0;
        self.page_scroll.scroll_to_item(0);
    }

    fn go_back(&mut self) {
        if let Some((p, r, c)) = self.back.pop() {
            self.page = p;
            self.row = r;
            self.col = c;
        }
    }

    fn goto_tab(&mut self, n: usize) {
        self.back.clear();
        self.row = 0;
        self.col = 0;
        self.page = if n == 0 { Page::Home } else { Page::Library(n - 1) };
    }

    // ---- lists the cursor walks ----

    fn b_list(&self) -> Vec<(Option<&'static str>, usize)> {
        match self.tab {
            0 => self
                .cat
                .sections
                .iter()
                .flat_map(|s| s.items.iter().enumerate().map(move |(i, &id)| ((i == 0).then_some(s.title), id)))
                .collect(),
            n => self.cat.libraries[n - 1].items.iter().map(|&id| (None, id)).collect(),
        }
    }

    fn c_grid(&self) -> Vec<usize> {
        let q = self.query.to_lowercase();
        let ids: Vec<usize> = match self.tab {
            0 => self.cat.items.iter().filter(|i| i.kind != Kind::Episode).map(|i| i.id).collect(),
            n => self.cat.libraries[n - 1].items.clone(),
        };
        ids.into_iter().filter(|&id| self.item(id).title.to_lowercase().contains(&q)).collect()
    }

    fn show_strip(&self) -> bool {
        self.tab == 0 && self.query.is_empty()
    }

    // ---- keys ----

    fn key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = ev.keystroke.key.as_str();
        match k {
            "f1" | "f2" | "f3" => {
                self.variant = k[1..].parse::<usize>().unwrap() - 1;
                *self = Self { focus: self.focus.clone(), ..Self::new(self.variant, cx) };
                return cx.notify();
            }
            _ => {}
        }
        self.status.clear();
        match self.variant {
            0 => self.key_a(ev),
            1 => self.key_b(ev),
            _ => self.key_c(ev),
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn dir(k: &str) -> Option<(i32, i32)> {
        Some(match k {
            "left" | "h" => (0, -1),
            "right" | "l" => (0, 1),
            "up" | "k" => (-1, 0),
            "down" | "j" => (1, 0),
            _ => return None,
        })
    }

    fn key_a(&mut self, ev: &KeyDownEvent) {
        let k = ev.keystroke.key.as_str();
        match (k, self.page) {
            ("1" | "2" | "3", _) => self.goto_tab(k.parse::<usize>().unwrap() - 1),
            ("escape" | "backspace", _) => self.go_back(),
            (_, Page::Home) => {
                let secs = self.cat.sections.len();
                if let Some((dr, dc)) = Self::dir(k) {
                    self.row = (self.row as i32 + dr).clamp(0, secs as i32 - 1) as usize;
                    let len = self.cat.sections[self.row].items.len();
                    self.col = if dr != 0 { self.col.min(len - 1) } else { (self.col as i32 + dc).clamp(0, len as i32 - 1) as usize };
                    self.page_scroll.scroll_to_item(self.row + 1);
                    self.row_scrolls[self.row].scroll_to_item(self.col);
                } else if k == "enter" {
                    let id = self.cat.sections[self.row].items[self.col];
                    self.open(Page::Detail(id));
                } else if k == "p" {
                    self.play(self.cat.sections[self.row].items[self.col]);
                }
            }
            (_, Page::Library(l)) => {
                let len = self.cat.libraries[l].items.len();
                if let Some((dr, dc)) = Self::dir(k) {
                    let ix = self.col as i32 + dr * self.cols as i32 + dc;
                    self.col = ix.clamp(0, len as i32 - 1) as usize;
                    self.grid_scroll.scroll_to_item(self.col);
                } else if k == "enter" {
                    self.open(Page::Detail(self.cat.libraries[l].items[self.col]));
                }
            }
            (_, Page::Detail(id)) => {
                let series = self.item(id).kind == Kind::Series;
                if let Some((dr, dc)) = Self::dir(k) {
                    if dr != 0 && series {
                        self.row = (self.row as i32 + dr).clamp(0, 1) as usize;
                        self.col = 0;
                    }
                    let len = if self.row == 0 { 3 } else { 8 };
                    self.col = (self.col as i32 + dc).clamp(0, len - 1) as usize;
                } else if k == "enter" || k == "p" {
                    if self.row == 0 && self.col > 0 {
                        self.status = ["", "Start over", "Added to watchlist"][self.col].into();
                    } else {
                        self.play(id);
                    }
                }
            }
        }
    }

    fn key_b(&mut self, ev: &KeyDownEvent) {
        let k = ev.keystroke.key.as_str();
        let len = self.b_list().len();
        match k {
            "1" | "2" | "3" => {
                self.tab = k.parse::<usize>().unwrap() - 1;
                self.sel = 0;
            }
            "tab" => {
                self.tab = (self.tab + 1) % 3;
                self.sel = 0;
            }
            "down" | "j" => self.sel = (self.sel + 1).min(len - 1),
            "up" | "k" => self.sel = self.sel.saturating_sub(1),
            "home" | "g" => self.sel = 0,
            "end" | "G" => self.sel = len - 1,
            "enter" | "p" => self.play(self.b_list()[self.sel].1),
            "w" => self.status = format!("Added “{}” to watchlist", self.item(self.b_list()[self.sel].1).title),
            _ => {}
        }
        self.grid_scroll.scroll_to_item(self.sel);
    }

    fn key_c(&mut self, ev: &KeyDownEvent) {
        let k = ev.keystroke.key.as_str();
        if self.typing {
            match k {
                "escape" => {
                    self.typing = false;
                    self.query.clear();
                }
                "enter" | "down" => self.typing = false,
                "backspace" => {
                    self.query.pop();
                }
                _ => {
                    if let Some(ch) = &ev.keystroke.key_char {
                        self.query.push_str(ch);
                    }
                }
            }
            self.row = 1;
            self.col = 0;
            return;
        }
        if let Some(id) = self.sheet {
            match k {
                "escape" | "backspace" | "q" => self.sheet = None,
                "enter" | "p" => self.play(id),
                "w" => self.status = format!("Added “{}” to watchlist", self.item(id).title),
                _ => {}
            }
            return;
        }
        let strip = self.cat.sections[0].items.clone();
        let grid = self.c_grid();
        match k {
            "/" => self.typing = true,
            "1" | "2" | "3" | "tab" => {
                self.tab = if k == "tab" { (self.tab + 1) % 3 } else { k.parse::<usize>().unwrap() - 1 };
                self.row = if self.show_strip() { 0 } else { 1 };
                self.col = 0;
            }
            "escape" => self.query.clear(),
            "enter" => {
                self.sheet = if self.row == 0 { strip.get(self.col).copied() } else { grid.get(self.col).copied() };
            }
            "p" => {
                let id = if self.row == 0 { strip.get(self.col) } else { grid.get(self.col) };
                if let Some(&id) = id {
                    self.play(id)
                }
            }
            _ => {
                if let Some((dr, dc)) = Self::dir(k) {
                    if self.row == 0 {
                        if dr > 0 {
                            self.row = 1;
                            self.col = self.col.min(self.cols - 1);
                        } else {
                            self.col = (self.col as i32 + dc).clamp(0, strip.len() as i32 - 1) as usize;
                        }
                    } else if dr < 0 && self.col < self.cols && self.show_strip() {
                        self.row = 0;
                        self.col = self.col.min(strip.len() - 1);
                        self.page_scroll.scroll_to_item(0);
                    } else if !grid.is_empty() {
                        let ix = self.col as i32 + dr * self.cols as i32 + dc;
                        self.col = ix.clamp(0, grid.len() as i32 - 1) as usize;
                        self.grid_scroll.scroll_to_item(self.col);
                    }
                }
            }
        }
    }
}

// ---- shared bits ----

fn sub_label(it: &Item) -> String {
    match it.kind {
        Kind::Movie => format!("{} · {}h {:02}m", it.year, it.runtime_min / 60, it.runtime_min % 60),
        Kind::Series => format!("{} · Series", it.year),
        Kind::Episode => it.subtitle.to_string(),
    }
}

fn meta_line(it: &Item) -> String {
    match it.kind {
        Kind::Series => format!("{} · {} · {} · {}", it.year, it.rating, it.subtitle, it.genres),
        _ => format!("{} · {} · {}h {:02}m · {}", it.year, it.rating, it.runtime_min / 60, it.runtime_min % 60, it.genres),
    }
}

/// Placeholder artwork: a gradient in the item's hue with the title on it.
fn art(it: &Item, w: f32, h: f32, radius: f32, selected: bool, t: &Theme) -> Div {
    let base = hsla(it.hue, 0.45, 0.34, 1.);
    let dark = hsla((it.hue + 0.06) % 1., 0.5, 0.13, 1.);
    div()
        .w(px(w))
        .h(px(h))
        .flex_shrink_0()
        .rounded(px(radius))
        .overflow_hidden()
        .bg(linear_gradient(165., linear_color_stop(base, 0.), linear_color_stop(dark, 1.)))
        .border_2()
        .border_color(if selected { t.accent } else { gpui_kit::transparent_black() })
        .flex()
        .flex_col()
        .justify_end()
        .p(px(8.))
        .gap(px(6.))
        .when(w >= 100., |d| d.child(
            div()
                .text_size(px((h / 10.).clamp(11., 22.)))
                .font_weight(FontWeight::BOLD)
                .text_color(hsla(0., 0., 1., 0.85))
                .child(it.title),
        ))
        .when_some(it.progress, |d, p| {
            d.child(
                div()
                    .h(px(3.))
                    .w_full()
                    .rounded_full()
                    .bg(hsla(0., 0., 0., 0.45))
                    .child(div().h_full().w(gpui_kit::relative(p)).rounded_full().bg(t.accent)),
            )
        })
}

fn card(it: &Item, w: f32, h: f32, radius: f32, selected: bool, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .w(px(w))
        .flex_shrink_0()
        .child(art(it, w, h, radius, selected, t))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if selected { t.bright } else { t.foreground })
                .truncate()
                .child(it.title),
        )
        .child(div().text_size(px(12.)).text_color(t.secondary).truncate().child(sub_label(it)))
}

fn pill(label: &str, primary: bool, selected: bool, t: &Theme) -> Div {
    div()
        .px(px(18.))
        .py(px(9.))
        .rounded_full()
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .bg(if primary { t.foreground } else { hsla(0., 0., 1., 0.1) })
        .text_color(if primary { t.background } else { t.foreground })
        .border_2()
        .border_color(if selected { t.accent } else { gpui_kit::transparent_black() })
        .child(label.to_string())
}

fn square_button(label: &str, primary: bool, t: &Theme) -> Div {
    div()
        .px(px(12.))
        .py(px(6.))
        .text_size(px(13.))
        .border_1()
        .border_color(if primary { t.accent } else { t.border })
        .bg(if primary { t.accent } else { t.surface })
        .text_color(if primary { t.on_accent } else { t.foreground })
        .child(label.to_string())
}

fn hint(keys: &[&str], label: &str, cx: &App) -> Div {
    let t = cx.omarchy();
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .children(keys.iter().map(|k| keycap(k.to_string(), cx)))
        .child(div().text_size(px(12.)).text_color(t.secondary).child(label.to_string()))
}

fn episodes(_series: &Item) -> Vec<(String, f32)> {
    (1..=8)
        .map(|e| (format!("E{e} · Episode title {e}"), if e < 3 { 1. } else if e == 3 { 0.4 } else { 0. }))
        .collect()
}

// ---- variant A: web clone ----

impl Proto {
    fn render_a(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.omarchy().clone();
        let detail = matches!(self.page, Page::Detail(_));
        let rail = detail;
        let nav_sel = match self.page {
            Page::Home => 0,
            Page::Library(l) => l + 1,
            Page::Detail(_) => 9,
        };
        let entries = [("⌂", "Home"), ("▤", "Movies"), ("▣", "Shows"), ("⌕", "Search"), ("☆", "Watchlist"), ("↺", "History")];
        let sidebar = div()
            .w(px(if rail { 64. } else { 240. }))
            .h_full()
            .flex_shrink_0()
            .bg(t.inset)
            .border_r_1()
            .border_color(t.divider())
            .flex()
            .flex_col()
            .p(px(14.))
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(t.bright)
                    .pb(px(14.))
                    .child(if rail { "s" } else { "silo" }),
            )
            .children(entries.iter().enumerate().map(|(i, (icon, name))| {
                let on = i == nav_sel;
                div()
                    .flex()
                    .gap(px(12.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .when(on, |d| d.bg(t.selected_fill()))
                    .text_color(if on { t.bright } else if i > 2 { t.secondary } else { t.foreground })
                    .child(icon.to_string())
                    .when(!rail, |d| {
                        d.child(name.to_string()).when(i < 3, |d| {
                            d.child(div().flex_1()).child(keycap((i + 1).to_string(), cx))
                        })
                    })
            }))
            .child(div().flex_1())
            .when(!rail, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().size(px(28.)).rounded_full().bg(t.accent))
                        .child(div().flex().flex_col().child("soup").child(div().text_size(px(11.)).text_color(t.secondary).child("silo.home.lan"))),
                )
            });
        let content_w = window.viewport_size().width.as_f32() - if rail { 64. } else { 240. } - 80.;
        let body = match self.page {
            Page::Home => self.a_home(&t),
            Page::Library(l) => self.a_library(l, content_w, &t),
            Page::Detail(id) => self.a_detail(id, &t),
        };
        div().size_full().flex().child(sidebar).child(body).into_any_element()
    }

    fn a_home(&mut self, t: &Theme) -> AnyElement {
        let hero = self.item(2).clone();
        let hero = div()
            .h(px(340.))
            .w_full()
            .flex_shrink_0()
            .bg(linear_gradient(
                90.,
                linear_color_stop(t.background, 0.),
                linear_color_stop(hsla(hero.hue, 0.45, 0.3, 1.), 1.),
            ))
            .flex()
            .flex_col()
            .justify_end()
            .px(px(40.))
            .pb(px(32.))
            .gap(px(10.))
            .child(div().text_size(px(11.)).text_color(t.secondary).child("FEATURED — NO. 01"))
            .child(div().text_size(px(44.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child(hero.title))
            .child(div().text_color(t.secondary).child(meta_line(&hero)))
            .child(div().max_w(px(560.)).text_color(t.foreground).child(hero.overview))
            .child(div().flex().gap(px(10.)).child(pill("▶ Resume", true, false, t)).child(pill("More info", false, false, t)));
        let rows = self.cat.sections.iter().enumerate().map(|(r, s)| {
            let (w, h) = if s.landscape { (290., 163.) } else { (150., 225.) };
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .px(px(40.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(12.))
                        .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child(s.title))
                        .child(div().text_size(px(11.)).text_color(t.secondary).child("VIEW ALL")),
                )
                .child(
                    div()
                        .id(("row", r))
                        .flex()
                        .gap(px(14.))
                        .overflow_x_scroll()
                        .track_scroll(&self.row_scrolls[r])
                        .children(s.items.iter().enumerate().map(|(c, &id)| {
                            card(&self.cat.items[id], w, h, 12., r == self.row && c == self.col, t)
                        })),
                )
        });
        div()
            .id("home")
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(36.))
            .pb(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .child(hero)
            .children(rows)
            .into_any_element()
    }

    fn a_library(&mut self, l: usize, width: f32, t: &Theme) -> AnyElement {
        self.cols = ((width + 12.) / (150. + 12.)).floor().max(1.) as usize;
        let lib = &self.cat.libraries[l];
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .px(px(40.))
            .pt(px(28.))
            .gap(px(18.))
            .child(div().text_size(px(34.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child(lib.name))
            .child(
                div()
                    .flex()
                    .gap(px(22.))
                    .text_size(px(14.))
                    .child(div().text_color(t.secondary).child("Recommended"))
                    .child(div().text_color(t.bright).border_b_2().border_color(t.foreground).child("Library"))
                    .child(div().text_color(t.secondary).child("Collections")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(pill("All media ▾", false, false, t))
                    .child(pill("Date added ▾", false, false, t))
                    .child(pill("Filters", false, false, t))
                    .child(div().flex_1())
                    .child(div().text_color(t.secondary).text_size(px(13.)).child(format!("{} items", lib.items.len()))),
            )
            .child(
                div()
                    .id("grid")
                    .flex_1()
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap(px(12.))
                    .pb(px(80.))
                    .overflow_y_scroll()
                    .track_scroll(&self.grid_scroll)
                    .children(lib.items.iter().enumerate().map(|(i, &id)| card(&self.cat.items[id], 150., 225., 12., i == self.col, t))),
            )
            .into_any_element()
    }

    fn a_detail(&mut self, id: usize, t: &Theme) -> AnyElement {
        let it = self.item(id).clone();
        let series = it.kind == Kind::Series;
        let actions = [if it.progress.is_some() { "▶ Resume" } else { "▶ Play" }, "Start over", "+ Watchlist"];
        let hero = div()
            .h(px(420.))
            .w_full()
            .flex_shrink_0()
            .bg(linear_gradient(
                180.,
                linear_color_stop(hsla(it.hue, 0.45, 0.3, 1.), 0.),
                linear_color_stop(t.background, 1.),
            ))
            .flex()
            .items_end()
            .gap(px(32.))
            .px(px(48.))
            .pb(px(28.))
            .child(art(&it, 200., 300., 12., false, t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .max_w(px(640.))
                    .child(div().text_size(px(11.)).text_color(t.secondary).child(if series { "SERIES · STUDIO" } else { "MOVIE · STUDIO" }))
                    .child(div().text_size(px(46.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child(it.title))
                    .child(div().text_color(t.secondary).child(meta_line(&it)))
                    .child(div().flex().gap(px(6.)).children(["4K", "HDR10", "Atmos"].map(|b| {
                        div().px(px(8.)).py(px(2.)).rounded_full().bg(hsla(0., 0., 1., 0.1)).text_size(px(10.)).child(b)
                    })))
                    .child(div().text_color(t.foreground).child(it.overview))
                    .child(div().flex().gap(px(10.)).children(
                        actions.iter().enumerate().map(|(i, a)| pill(a, i == 0, self.row == 0 && self.col == i, t)),
                    )),
            );
        let below = if series {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .px(px(48.))
                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child("Season 1 ▾"))
                .child(div().flex().flex_wrap().gap(px(14.)).children(episodes(&it).into_iter().enumerate().map(|(i, (name, p))| {
                    let ep = Item { title: "", progress: (p > 0.).then_some(p), hue: (it.hue + i as f32 * 0.02) % 1., ..it.clone() };
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .w(px(250.))
                        .child(art(&ep, 250., 140., 10., self.row == 1 && self.col == i, t))
                        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(div().text_size(px(12.)).text_color(t.secondary).child("42m · A short synopsis on two lines."))
                })))
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .px(px(48.))
                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child("Cast"))
                .child(div().flex().gap(px(16.)).children((0..8).map(|i| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(div().w(px(110.)).h(px(150.)).rounded(px(10.)).bg(t.surface))
                        .child(div().text_size(px(12.)).child(format!("Actor {}", i + 1)))
                        .child(div().text_size(px(11.)).text_color(t.secondary).child("Role"))
                })))
        };
        div()
            .id("detail")
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(32.))
            .pb(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .child(hero)
            .child(below)
            .into_any_element()
    }
}

// ---- variant B: split pane ----

impl Proto {
    fn render_b(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.omarchy().clone();
        let list = self.b_list();
        let sel_id = list[self.sel.min(list.len() - 1)].1;
        let tabs = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(12.))
            .h(px(40.))
            .border_b_1()
            .border_color(t.divider())
            .children(["Home", "Movies", "Shows"].iter().enumerate().map(|(i, name)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .h_full()
                    .border_b_2()
                    .border_color(if i == self.tab { t.accent } else { gpui_kit::transparent_black() })
                    .text_color(if i == self.tab { t.bright } else { t.secondary })
                    .child(keycap((i + 1).to_string(), cx))
                    .child(name.to_string())
            }))
            .child(div().flex_1())
            .child(div().text_size(px(12.)).text_color(t.secondary).child("soup @ silo.home.lan"));
        let rows = list.iter().enumerate().map(|(i, &(header, id))| {
            let it = &self.cat.items[id];
            let on = i == self.sel;
            div()
                .flex()
                .flex_col()
                .when_some(header, |d, h| {
                    d.child(
                        div()
                            .px(px(12.))
                            .pt(px(14.))
                            .pb(px(6.))
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.accent)
                            .child(h.to_uppercase()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(12.))
                        .py(px(5.))
                        .when(on, |d| d.bg(t.selected_fill()))
                        .border_l_2()
                        .border_color(if on { t.accent } else { gpui_kit::transparent_black() })
                        .child(art(it, 34., 50., 0., false, &t).p(px(2.)))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(div().truncate().text_color(if on { t.bright } else { t.foreground }).child(it.title))
                                .child(div().text_size(px(12.)).text_color(t.secondary).child(sub_label(it))),
                        )
                        .when_some(it.progress, |d, p| {
                            d.child(div().w(px(48.)).h(px(4.)).bg(t.border).child(div().h_full().w(gpui_kit::relative(p)).bg(t.accent)))
                        }),
                )
        });
        let left = div()
            .id("list")
            .w(px(420.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(t.divider())
            .overflow_y_scroll()
            .track_scroll(&self.grid_scroll)
            .pb(px(80.))
            .children(rows);
        let it = self.item(sel_id).clone();
        let series = it.kind == Kind::Series;
        let right = div()
            .id("pane")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(200.))
                    .flex_shrink_0()
                    .bg(linear_gradient(180., linear_color_stop(hsla(it.hue, 0.4, 0.3, 1.), 0.), linear_color_stop(t.background, 1.))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .px(px(28.))
                    .mt(px(-120.))
                    .child(art(&it, 160., 240., 0., false, &t))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .gap(px(8.))
                            .child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).text_color(t.bright).child(it.title))
                            .child(div().text_color(t.secondary).child(meta_line(&it)))
                            .child(div().flex().gap(px(8.)).child(square_button(if it.progress.is_some() { "Resume" } else { "Play" }, true, &t)).child(square_button("Watchlist", false, &t))),
                    ),
            )
            .child(div().px(px(28.)).pt(px(18.)).max_w(px(720.)).child(it.overview))
            .when(series, |d| {
                d.child(div().px(px(28.)).pt(px(22.)).pb(px(6.)).font_weight(FontWeight::BOLD).text_color(t.accent).child("SEASON 1"))
                    .children(episodes(&it).into_iter().map(|(name, p)| {
                        div()
                            .flex()
                            .gap(px(12.))
                            .px(px(28.))
                            .py(px(4.))
                            .child(div().w(px(16.)).text_color(if p >= 1. { t.success } else { t.secondary }).child(if p >= 1. { "✓" } else if p > 0. { "◐" } else { "·" }))
                            .child(name)
                    }))
            })
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .px(px(28.))
                    .pt(px(24.))
                    .pb(px(80.))
                    .child(hint(&["j", "k"], "move", cx))
                    .child(hint(&["⏎"], "play", cx))
                    .child(hint(&["w"], "watchlist", cx))
                    .child(hint(&["⇥"], "next tab", cx)),
            );
        div().size_full().flex().flex_col().child(tabs).child(div().flex_1().min_h_0().flex().child(left).child(right)).into_any_element()
    }
}

// ---- variant C: grid + sheet ----

impl Proto {
    fn render_c(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.omarchy().clone();
        let w = window.viewport_size().width.as_f32() - 48.;
        self.cols = ((w + 14.) / (160. + 14.)).floor().max(1.) as usize;
        let grid = self.c_grid();
        let bar = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(24.))
            .h(px(52.))
            .border_b_1()
            .border_color(t.divider())
            .child(
                div()
                    .w(px(320.))
                    .px(px(10.))
                    .py(px(6.))
                    .border_1()
                    .border_color(if self.typing { t.accent } else { t.border })
                    .bg(t.inset)
                    .text_color(if self.query.is_empty() && !self.typing { t.secondary } else { t.foreground })
                    .child(if self.query.is_empty() && !self.typing {
                        "/  Filter titles".to_string()
                    } else {
                        format!("{}{}", self.query, if self.typing { "▏" } else { "" })
                    }),
            )
            .children(["All", "Movies", "Shows"].iter().enumerate().map(|(i, n)| {
                div()
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .px(px(10.))
                    .py(px(5.))
                    .border_1()
                    .border_color(if i == self.tab { t.accent } else { t.border })
                    .bg(if i == self.tab { t.accent } else { t.surface })
                    .text_color(if i == self.tab { t.on_accent } else { t.foreground })
                    .child(n.to_string())
            }))
            .child(div().flex_1())
            .child(div().text_size(px(12.)).text_color(t.secondary).child(format!("{} titles", grid.len())));
        let strip = self.show_strip().then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(24.))
                .pt(px(16.))
                .child(div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.accent).child("CONTINUE"))
                .child(div().flex().gap(px(14.)).children(self.cat.sections[0].items.iter().enumerate().map(|(i, &id)| {
                    let it = &self.cat.items[id];
                    div()
                        .flex()
                        .gap(px(10.))
                        .items_center()
                        .p(px(6.))
                        .border_1()
                        .border_color(if self.row == 0 && self.col == i { t.accent } else { t.border })
                        .bg(t.surface)
                        .child(art(it, 120., 68., 0., false, &t))
                        .child(div().flex().flex_col().w(px(150.)).child(div().truncate().child(it.title)).child(div().truncate().text_size(px(12.)).text_color(t.secondary).child(sub_label(it))))
                })))
        });
        let body = div()
            .id("grid")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(14.))
            .px(px(24.))
            .pt(px(16.))
            .pb(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.grid_scroll)
            .children(grid.iter().enumerate().map(|(i, &id)| card(&self.cat.items[id], 160., 240., 0., self.row == 1 && self.col == i, &t)));
        let sheet = self.sheet.map(|id| {
            let it = self.item(id).clone();
            div()
                .absolute()
                .inset_0()
                .bg(hsla(0., 0., 0., 0.6))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(820.))
                        .max_h(px(620.))
                        .bg(t.surface)
                        .border_1()
                        .border_color(t.accent)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(220.))
                                .bg(linear_gradient(180., linear_color_stop(hsla(it.hue, 0.45, 0.32, 1.), 0.), linear_color_stop(t.surface, 1.)))
                                .flex()
                                .items_end()
                                .p(px(24.))
                                .child(div().text_size(px(36.)).font_weight(FontWeight::BOLD).text_color(t.bright).child(it.title)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(12.))
                                .px(px(24.))
                                .pb(px(24.))
                                .child(div().text_color(t.secondary).child(meta_line(&it)))
                                .child(it.overview)
                                .when(it.kind == Kind::Series, |d| {
                                    d.child(div().flex().flex_wrap().gap(px(6.)).children(episodes(&it).into_iter().map(|(n, p)| {
                                        div().px(px(8.)).py(px(3.)).border_1().border_color(if p >= 1. { t.success } else { t.border }).text_size(px(12.)).child(n)
                                    })))
                                })
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(16.))
                                        .items_center()
                                        .child(square_button(if it.progress.is_some() { "Resume" } else { "Play" }, true, &t))
                                        .child(hint(&["⏎"], "play", cx))
                                        .child(hint(&["w"], "watchlist", cx))
                                        .child(hint(&["esc"], "close", cx)),
                                ),
                        ),
                )
        });
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(bar)
            .children(strip)
            .child(body)
            .children(sheet)
            .into_any_element()
    }
}

impl Render for Proto {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let content = match self.variant {
            0 => self.render_a(window, cx),
            1 => self.render_b(cx),
            _ => self.render_c(window, cx),
        };
        let keys = match self.variant {
            0 => "1-3 pages · hjkl/arrows move · ⏎ open · p play · esc back",
            1 => "1-3 / ⇥ tabs · j/k move · ⏎ play · w watchlist",
            _ => "/ filter · 1-3 / ⇥ scope · hjkl move · ⏎ sheet · p play · esc close",
        };
        // Prototype switcher: visibly not part of the design.
        let switcher = div()
            .absolute()
            .bottom(px(14.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(8.))
                    .rounded_full()
                    .bg(hsla(0.13, 0.95, 0.55, 1.))
                    .text_color(hsla(0., 0., 0., 1.))
                    .text_size(px(13.))
                    .shadow_lg()
                    .child(
                        div().id("prev").cursor_pointer().child("◀").on_click(cx.listener(|this, _, _, cx| {
                            let v = (this.variant + 2) % 3;
                            *this = Self { focus: this.focus.clone(), ..Self::new(v, cx) };
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("F{} {} · {}", self.variant + 1, ["A", "B", "C"][self.variant], NAMES[self.variant])),
                    )
                    .child(
                        div().id("next").cursor_pointer().child("▶").on_click(cx.listener(|this, _, _, cx| {
                            let v = (this.variant + 1) % 3;
                            *this = Self { focus: this.focus.clone(), ..Self::new(v, cx) };
                            cx.notify();
                        })),
                    )
                    .child(div().child(if self.status.is_empty() { keys.to_string() } else { format!("→ {}", self.status) })),
            );
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font.clone())
            .text_size(px(14.))
            .child(content)
            .child(switcher)
    }
}

fn main() {
    let variant = match std::env::args().nth(1).as_deref() {
        Some("b") => 1,
        Some("c") => 2,
        _ => 0,
    };
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_omarchy::init(cx);
        cx.open_window(
            WindowOptions {
                app_id: Some("silo-omarchy".into()),
                titlebar: None,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1400.), px(900.)), cx))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| Proto::new(variant, cx));
                let focus = view.read(cx).focus.clone();
                window.focus(&focus, cx);
                view
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}

