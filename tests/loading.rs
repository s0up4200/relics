//! The loader contract through the result and loading state that a page displays.
use std::future::Future;

use futures::channel::oneshot;
use gpui::{AppContext as _, Context, TestAppContext};
use relics::api::Error;
use relics::pages::loading::{Failure, Loader};

#[derive(Default)]
struct Page {
    load: Loader,
    content: Vec<&'static str>,
    error: Option<String>,
}

impl Page {
    fn fetch(
        &mut self,
        policy: Failure,
        response: impl Future<Output = Result<Vec<&'static str>, Error>> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        Loader::run(
            self,
            cx,
            |page| &mut page.load,
            policy,
            response,
            |page, result, _| match result {
                Ok(content) => page.content = content,
                Err(error) => page.error = Some(error.to_string()),
            },
            |_, _| panic!("no refresh was queued"),
        );
    }

    fn display(&self) -> String {
        if let Some(error) = &self.error {
            format!("{error}: Retry")
        } else if self.load.visible_loading() {
            "Loading…".into()
        } else if self.content.is_empty() {
            "No entries.".into()
        } else {
            self.content.join(", ")
        }
    }
}

#[gpui::test]
fn empty_success_survives_a_failed_background_reload(cx: &mut TestAppContext) {
    let page = cx.new(|_| Page::default());
    page.update(cx, |page, cx| page.fetch(Failure::Retain, async { Ok(vec![]) }, cx));
    cx.run_until_parked();
    assert_eq!(page.read_with(cx, |page, _| page.display()), "No entries.");
    let (release, response) = oneshot::channel();
    page.update(cx, |page, cx| page.fetch(Failure::Retain, async { response.await.unwrap() }, cx));
    assert_eq!(page.read_with(cx, |page, _| page.display()), "No entries.");
    release.send(Err(Error::Status(500))).unwrap();
    cx.run_until_parked();
    assert_eq!(page.read_with(cx, |page, _| page.display()), "No entries.");
}

#[gpui::test]
fn retry_shows_loading_and_can_fail_again_before_recovery(cx: &mut TestAppContext) {
    let page = cx.new(|_| Page::default());
    for _ in 0..2 {
        let (release, response) = oneshot::channel();
        page.update(cx, |page, cx| page.fetch(Failure::Retain, async { response.await.unwrap() }, cx));
        assert_eq!(page.read_with(cx, |page, _| page.display()), "Loading…");
        release.send(Err(Error::Status(500))).unwrap();
        cx.run_until_parked();
        assert!(page.read_with(cx, |page, _| page.display()).ends_with(": Retry"));
    }
    page.update(cx, |page, cx| page.fetch(Failure::Retain, async { Ok(vec!["Recovered"]) }, cx));
    cx.run_until_parked();
    assert_eq!(page.read_with(cx, |page, _| page.display()), "Recovered");
}

#[gpui::test]
fn pagination_failure_keeps_entries_and_optional_failure_keeps_its_row(cx: &mut TestAppContext) {
    let page = cx.new(|_| Page::default());
    page.update(cx, |page, cx| page.fetch(Failure::Retain, async { Ok(vec!["First"]) }, cx));
    cx.run_until_parked();
    page.update(cx, |page, cx| page.fetch(Failure::Page, async { Err(Error::Status(500)) }, cx));
    cx.run_until_parked();
    assert!(page.read_with(cx, |page, _| page.display()).ends_with(": Retry"));
    assert_eq!(page.read_with(cx, |page, _| page.content.clone()), ["First"]);
    page.update(cx, |page, cx| page.fetch(Failure::Quiet, async { Err(Error::Status(500)) }, cx));
    cx.run_until_parked();
    assert_eq!(page.read_with(cx, |page, _| page.display()), "First");
}

#[gpui::test]
fn a_new_choice_rejects_the_previous_response_and_its_success(cx: &mut TestAppContext) {
    let page = cx.new(|_| Page::default());
    page.update(cx, |page, cx| page.fetch(Failure::Retain, async { Ok(vec!["Old"]) }, cx));
    cx.run_until_parked();
    let (release, response) = oneshot::channel::<Result<Vec<&str>, Error>>();
    page.update(cx, |page, cx| page.fetch(Failure::Retain, async { response.await.unwrap() }, cx));
    cx.run_until_parked();
    page.update(cx, |page, cx| {
        page.load.clear();
        page.content.clear();
        page.fetch(Failure::Retain, async { Err(Error::Status(500)) }, cx);
    });
    cx.run_until_parked();
    release.send(Ok(vec!["Obsolete"])).ok();
    cx.run_until_parked();
    assert!(page.read_with(cx, |page, _| page.display()).ends_with(": Retry"));
}
