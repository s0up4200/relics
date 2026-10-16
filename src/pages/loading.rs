//! Shared load lifetime and failure rules. Pages own the data and its display.

use std::future::Future;
use std::ops::{Deref, DerefMut};

use gpui::{Context, Task};

use super::freshness::Fresh;
use crate::api::Error;

#[derive(Clone, Copy)]
pub enum Failure {
    /// Keep every successful result, including an empty result, on a failed reload.
    Retain,
    /// A missing page has its own Retry, even when earlier pages loaded.
    Page,
    /// Optional content never shows an error.
    Quiet,
}

pub struct Loader {
    fresh: Fresh,
    loaded: bool,
    task: Task<()>,
    foreground: bool,
}

impl Default for Loader {
    fn default() -> Self {
        Self { fresh: Fresh::default(), loaded: false, task: Task::ready(()), foreground: false }
    }
}

impl Deref for Loader {
    type Target = Fresh;

    fn deref(&self) -> &Fresh {
        &self.fresh
    }
}

impl DerefMut for Loader {
    fn deref_mut(&mut self) -> &mut Fresh {
        &mut self.fresh
    }
}

impl Loader {
    pub fn visible_loading(&self) -> bool {
        self.fresh.loading() && self.foreground
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Page status text. Loading belongs to this operation, rather than its retained result.
    pub fn status<'a>(&self, text: Option<&'a str>, failed: bool) -> Option<(&'a str, bool)> {
        if self.visible_loading() { Some(("Loading…", false)) } else { text.map(|text| (text, failed)) }
    }

    /// Discard the previous data choice and cancel its request.
    pub fn clear(&mut self) {
        self.fresh.clear();
        self.loaded = false;
        self.task = Task::ready(());
    }

    pub fn run<P: 'static, T: 'static>(
        page: &mut P,
        cx: &mut Context<P>,
        state: fn(&mut P) -> &mut Self,
        failure: Failure,
        fetch: impl Future<Output = Result<T, Error>> + 'static,
        merge: impl FnOnce(&mut P, Result<T, Error>, &mut Context<P>) + 'static,
        reload: fn(&mut P, &mut Context<P>),
    ) {
        Self::run_checked(page, cx, state, failure, fetch, |_, _| true, merge, reload);
    }

    /// Child requests also belong to the parent load and its writes.
    #[allow(clippy::too_many_arguments)]
    pub fn run_checked<P: 'static, T: 'static>(
        page: &mut P,
        cx: &mut Context<P>,
        state: fn(&mut P) -> &mut Self,
        failure: Failure,
        fetch: impl Future<Output = Result<T, Error>> + 'static,
        accepts: impl FnOnce(&mut P, &mut Context<P>) -> bool + 'static,
        merge: impl FnOnce(&mut P, Result<T, Error>, &mut Context<P>) + 'static,
        reload: fn(&mut P, &mut Context<P>),
    ) {
        let loader = state(page);
        loader.foreground = !loader.loaded || matches!(failure, Failure::Page);
        let ticket = match failure {
            Failure::Page => loader.fresh.more(),
            _ => loader.fresh.begin(),
        };
        loader.task = cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |page, cx| {
                if state(page).fresh.end(ticket) && accepts(page, cx) {
                    let loader = state(page);
                    match &result {
                        Ok(_) => {
                            loader.loaded = true;
                            merge(page, result, cx);
                        }
                        Err(err)
                            if matches!(failure, Failure::Quiet)
                                || (matches!(failure, Failure::Retain) && loader.loaded) =>
                        {
                            eprintln!("relics: cannot reload page content: {err}");
                        }
                        Err(_) => merge(page, result, cx),
                    }
                }
                if state(page).fresh.ready() {
                    reload(page, cx);
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }
}
