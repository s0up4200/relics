use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window,
};
use gpui_kit::base::input::InputState;

struct Field(Entity<InputState>);
impl Render for Field {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui_omarchy::input("field", &self.0, window, cx)
    }
}

#[gpui::test]
fn super_shortcuts_edit_the_focused_input(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |input, cx| input.focus(window, cx));
        Field(input)
    });
    cx.simulate_input("café noir");
    cx.simulate_keystrokes("super-a");
    cx.simulate_input("heat");
    cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), "heat"));
}

mod common;
use common::{Log, fake, json, not_found};
use relics::pages::Pages;
use std::sync::Arc;
use std::time::Duration;

struct PageWindow(Entity<Pages>);
impl Render for PageWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_omarchy::focus_scope("test-pages").size_full().child(self.0.clone())
    }
}

fn client() -> (Arc<relics::api::Client>, Log) {
    let (client, seen) = fake(|request| {
        match request.path.split('?').next().unwrap() {
        "user/libraries" => common::ok("user_libraries.json"),
        "home/layout" | "library/12/layout" => json(200, r#"{"sections":[]}"#.into()),
        "home/sections" => json(200, r#"{"sections":[]}"#.into()),
        "catalog/filters" => json(200, r#"{"authors":[],"narrators":[],"series":[],"content_ratings":[],"genres":[],"studios":["A24","Neon","Annapurna"],"networks":[],"countries":[],"original_languages":[]}"#.into()),
        "catalog/people" if request.path.contains("q=person") => json(200, r#"{"items":[{"id":"7","name":"Test Person"}]}"#.into()),
        "catalog/people/7" => json(200, r#"{"id":"7","name":"Test Person"}"#.into()),
        "catalog/people" => json(200, r#"{"items":[]}"#.into()),
        "catalog/query" if request.body.contains("\"q\":\"item\"") => json(200, r#"{"items":[{"content_id":"test-item","type":"movie","title":"Test Item","genres":[],"keywords":[],"status":"matched"}],"total":1,"total_exact":true,"window_cursor":"test","page":{"has_more":false}}"#.into()),
        "catalog/query" => json(200, r#"{"items":[],"total":0,"total_exact":true,"window_cursor":"test","page":{"has_more":false}}"#.into()),
        _ => not_found(),
    }
    });
    client.set_profile(None);
    (Arc::new(client), seen)
}
fn queries(seen: &Log) -> Vec<serde_json::Value> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|r| r.path.starts_with("catalog/query"))
        .map(|r| serde_json::from_str(&r.body).unwrap())
        .collect()
}

#[gpui::test]
fn search_edits_use_one_delay_and_page_shortcuts_stay_in_the_field(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (client, seen) = client();
    let (_, cx) =
        cx.add_window_view(|window, cx| PageWindow(cx.new(|cx| Pages::new(client, "Test".into(), window, cx))));
    cx.simulate_keystrokes("/");
    cx.simulate_input("m1/hjkl café noir");
    cx.executor().advance_clock(Duration::from_millis(199));
    cx.run_until_parked();
    assert!(queries(&seen).is_empty());
    cx.simulate_keystrokes("super-a");
    cx.simulate_input("heat");
    cx.executor().advance_clock(Duration::from_millis(199));
    cx.run_until_parked();
    assert!(queries(&seen).is_empty());
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(queries(&seen).len(), 1);
    assert_eq!(queries(&seen)[0]["q"], "heat");
    cx.simulate_keystrokes("ctrl-a super-c left right");
    cx.executor().advance_clock(Duration::from_millis(250));
    cx.run_until_parked();
    assert_eq!(queries(&seen).len(), 1, "selection and copy do not search");
    cx.simulate_input("x");
    cx.simulate_keystrokes("escape");
    cx.executor().advance_clock(Duration::from_millis(250));
    cx.run_until_parked();
    assert_eq!(queries(&seen).len(), 1, "Escape cancels the pending search");
    cx.simulate_input("new");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(queries(&seen).last().unwrap()["q"], "new");
    cx.simulate_keystrokes("ctrl-a super-x");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let count = queries(&seen).len();
    for (key, before, after) in
        [("super-v", count, count + 1), ("super-z", count + 1, count + 1), ("super-shift-z", count + 1, count + 2)]
    {
        cx.simulate_keystrokes(key);
        cx.executor().advance_clock(Duration::from_millis(199));
        cx.run_until_parked();
        assert_eq!(queries(&seen).len(), before, "{key} waits for the pause");
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        assert_eq!(queries(&seen).len(), after, "{key} sends only the latest value");
    }
    assert_eq!(queries(&seen).last().unwrap()["q"], "new");
    assert_eq!(queries(&seen).len(), count + 2, "paste and redo each search once, undo clears");
}

#[gpui::test]
fn filter_drafts_edit_normally_and_reject_an_entire_mixed_year_paste(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (client, seen) = client();
    let (_, cx) =
        cx.add_window_view(|window, cx| PageWindow(cx.new(|cx| Pages::new(client, "Test".into(), window, cx))));
    cx.simulate_keystrokes("/");
    cx.simulate_input("heat");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    cx.simulate_keystrokes("down right enter down down down");
    cx.simulate_input("1995");
    cx.simulate_keystrokes("left backspace");
    cx.simulate_input("8");
    cx.simulate_keystrokes("enter");
    let before = queries(&seen);
    assert!(before.last().unwrap().to_string().contains("1985"), "Enter applies the edited year: {before:?}");
    cx.simulate_input("2000");
    cx.simulate_keystrokes("ctrl-a");
    cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("20x4".into())));
    cx.simulate_keystrokes("super-v enter");
    assert!(queries(&seen).last().unwrap().to_string().contains("2000"), "a rejected paste keeps the draft");
    for invalid in ["20\n24", "\n", "\r", "20\t24", "２０２４"] {
        cx.simulate_input("2000");
        cx.simulate_keystrokes("ctrl-a");
        cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(invalid.into())));
        cx.simulate_keystrokes("ctrl-v");
        cx.simulate_input("1999");
        cx.simulate_keystrokes("enter");
        assert_eq!(
            queries(&seen).last().unwrap()["groups"][0]["rules"][0]["value"],
            1999,
            "invalid paste preserves the selection: {invalid:?}"
        );
    }
    cx.simulate_keystrokes("enter");
    assert!(!queries(&seen).last().unwrap().to_string().contains("year"), "an empty draft removes the year filter");
    cx.simulate_keystrokes("down down down");
    cx.simulate_input("an");
    cx.simulate_keystrokes("enter");
    assert!(queries(&seen).last().unwrap().to_string().contains("Annapurna"));
    cx.simulate_input("missing");
    let count = queries(&seen).len();
    cx.simulate_keystrokes("enter");
    assert_eq!(queries(&seen).len(), count, "no matching Facet keeps the applied filter");
    cx.simulate_keystrokes("escape down");
    cx.simulate_input("  Tom Hanks  ");
    cx.simulate_keystrokes("enter");
    assert!(queries(&seen).last().unwrap().to_string().contains("Tom Hanks"));
}

#[gpui::test]
fn deletion_and_history_use_the_editor_in_both_directions(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |input, cx| input.focus(window, cx));
        Field(input)
    });
    for (keys, expected) in [
        ("home delete", "afé noir"),
        ("end backspace", "café noi"),
        ("home ctrl-delete", " noir"),
        ("end ctrl-backspace", "café "),
        ("home right right super-delete", "ca"),
        ("home right right super-backspace", "fé noir"),
        ("end ctrl-delete super-delete", "café noir"),
        ("home ctrl-backspace super-backspace", "café noir"),
        ("home shift-right shift-right super-delete", "fé noir"),
        ("ctrl-a backspace", ""),
        ("super-a delete", ""),
    ] {
        cx.update(|window, cx| {
            view.read(cx).0.clone().update(cx, |input, cx| input.set_value("café noir", window, cx))
        });
        cx.simulate_keystrokes(keys);
        cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), expected, "{keys}"));
    }
    cx.simulate_input("heat");
    cx.simulate_keystrokes("ctrl-a super-x");
    cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), ""));
    cx.simulate_keystrokes("super-v");
    cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), "heat"));
    cx.simulate_keystrokes("super-z");
    cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), ""));
    cx.simulate_keystrokes("super-shift-z");
    cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), "heat"));
    cx.simulate_keystrokes("ctrl-z ctrl-y");
    cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), "heat"));
}

#[gpui::test]
fn super_clipboard_shortcuts_keep_masked_text_private(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        input.update(cx, |input, cx| input.focus(window, cx));
        Field(input)
    });
    cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("public".into())));
    cx.simulate_input("secret");
    cx.simulate_keystrokes("super-a super-c super-x");
    cx.update(|_, cx| {
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "public");
        assert_eq!(view.read(cx).0.read(cx).value(), "secret");
    });
    cx.simulate_keystrokes("super-v");
    cx.update(|_, cx| assert_eq!(view.read(cx).0.read(cx).value(), "public"));
}

#[gpui::test]
fn search_focus_returns_after_back_navigation_and_enter_without_results(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (client, seen) = client();
    let (_, cx) =
        cx.add_window_view(|window, cx| PageWindow(cx.new(|cx| Pages::new(client, "Test".into(), window, cx))));
    cx.simulate_keystrokes("/ enter");
    cx.simulate_input("first");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(queries(&seen).last().unwrap()["q"], "first");
    cx.simulate_keystrokes("escape escape /");
    cx.simulate_input("second");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(queries(&seen).last().unwrap()["q"], "second");
}

#[gpui::test]
fn library_year_fields_follow_tab_focus_and_keep_empty_backspace_in_the_panel(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (client, seen) = client();
    let (_, cx) =
        cx.add_window_view(|window, cx| PageWindow(cx.new(|cx| Pages::new(client, "Test".into(), window, cx))));
    cx.run_until_parked();
    cx.simulate_keystrokes("2 up right enter down down down backspace");
    cx.simulate_input("2004");
    cx.simulate_keystrokes("enter tab");
    cx.simulate_input("2024");
    cx.simulate_keystrokes("left backspace");
    cx.simulate_input("1");
    cx.simulate_keystrokes("enter");
    let query = queries(&seen).pop().unwrap();
    assert_eq!(
        query["groups"][0]["rules"],
        serde_json::json!([
            {"field":"year","op":"gte","value":2004}, {"field":"year","op":"lte","value":2014}
        ]),
        "all queries: {:?}",
        queries(&seen)
    );
    assert!(query["q"].is_null());
}

#[gpui::test]
fn pin_enter_submits_the_edited_value_and_escape_returns_to_the_picker(cx: &mut TestAppContext) {
    use relics::profiles::ProfilePicker;
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (client, seen) = fake(|req| match req.path.as_str() {
        "profiles/p-owner/verify-pin" => common::ok("verify_profile_pin_ok.json"),
        _ => not_found(),
    });
    let mut profiles: relics::api::wire::ProfileCollection =
        serde_json::from_str(&common::fixture("list_profiles_ok.json")).unwrap();
    profiles.items[0].has_pin = true;
    profiles.items[0].avatar_url = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        ProfilePicker::new(Arc::new(client), profiles.items, Some("p-owner"), window, cx)
    });
    cx.simulate_input("1234");
    cx.simulate_keystrokes("escape");
    cx.simulate_input("ignored");
    assert!(seen.lock().unwrap().is_empty(), "Escape does not submit the PIN");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("1234");
    cx.simulate_keystrokes("super-a");
    cx.simulate_input("5678");
    cx.simulate_keystrokes("enter");
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&requests[0].body).unwrap(),
        serde_json::json!({"pin":"5678"})
    );
}

#[gpui::test]
fn search_enter_opens_available_people_and_items_and_restores_editing_after_back(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (client, seen) = client();
    let (_, cx) =
        cx.add_window_view(|window, cx| PageWindow(cx.new(|cx| Pages::new(client, "Test".into(), window, cx))));
    cx.simulate_keystrokes("/");
    cx.simulate_input("person");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter enter");
    assert!(seen.lock().unwrap().iter().any(|r| r.path == "catalog/people/7"));
    cx.simulate_keystrokes("escape / super-a");
    cx.simulate_input("item");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(queries(&seen).last().unwrap()["q"], "item");
    cx.simulate_keystrokes("enter enter");
    assert!(
        seen.lock().unwrap().iter().any(|r| r.path.starts_with("catalog/items/test-item?")),
        "requests: {:?}",
        seen.lock().unwrap().iter().map(|r| &r.path).collect::<Vec<_>>()
    );
}

#[gpui::test]
fn search_mouse_and_sidebar_entry_focus_the_real_editor(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let (client, seen) = client();
    let (_, cx) =
        cx.add_window_view(|window, cx| PageWindow(cx.new(|cx| Pages::new(client, "Test".into(), window, cx))));
    cx.simulate_keystrokes("/");
    cx.simulate_input("heat");
    cx.simulate_keystrokes("down");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let bounds = cx.debug_bounds("search-field").unwrap();
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    cx.simulate_keystrokes("home");
    cx.simulate_input("new");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(queries(&seen).last().unwrap()["q"], "newheat");
    cx.simulate_keystrokes("down left up up enter left down down enter super-a");
    cx.simulate_input("sidebar");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(queries(&seen).last().unwrap()["q"], "sidebar");
}

#[gpui::test]
fn closing_the_account_menu_restores_the_filter_field_and_selection(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    for search in [true, false] {
        for to_year in [false, true] {
            let (client, seen) = client();
            let (_, cx) =
                cx.add_window_view(|window, cx| PageWindow(cx.new(|cx| Pages::new(client, "Test".into(), window, cx))));
            cx.run_until_parked();
            if search {
                cx.simulate_keystrokes("/");
                cx.simulate_input("heat");
                cx.executor().advance_clock(Duration::from_millis(200));
                cx.run_until_parked();
                cx.simulate_keystrokes("down right enter down down down");
            } else {
                cx.simulate_keystrokes("2 up right enter down down down");
            }
            if to_year {
                cx.simulate_keystrokes("tab");
            }
            cx.simulate_input("2004");
            cx.simulate_keystrokes("home right shift-right");
            let count = queries(&seen).len();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let menu = cx.debug_bounds("account-menu").unwrap();
            cx.simulate_click(menu.center(), gpui::Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.simulate_keystrokes("escape");
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.simulate_input("9");
            assert_eq!(queries(&seen).len(), count, "editing does not apply the filter");
            cx.simulate_keystrokes("enter");
            let query = queries(&seen).pop().unwrap();
            assert_eq!(
                query["groups"][0]["rules"],
                serde_json::json!([
                    {"field":"year", "op": if to_year { "lte" } else { "gte" }, "value":2904}
                ]),
                "search={search}, to_year={to_year}"
            );
        }
    }
}
