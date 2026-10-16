//! The startup trace. When `RELICS_STARTUP_TRACE` holds the spawn time of the process as Unix epoch nanoseconds,
//! the app writes one line to stderr at each startup milestone: `relics: startup <milestone> <ms>`, with the whole
//! milliseconds since the spawn time. `scripts/startup-baseline.py` reads these lines.

use std::collections::HashSet;
use std::io::{Stderr, Write};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A startup trace that writes each milestone once to `sink`.
pub struct Trace<W> {
    /// The spawn time of the process. `None` turns the trace off.
    spawn: Option<SystemTime>,
    written: HashSet<&'static str>,
    sink: W,
}

impl<W: Write> Trace<W> {
    /// A trace from the env var `value`. A value that is missing or that is not a number turns the trace off.
    pub fn new(value: Option<&str>, sink: W) -> Self {
        // The spawn time comes from another process, so the trace compares it with the wall clock.
        let spawn = value.and_then(|value| value.parse().ok()).map(|nanos| UNIX_EPOCH + Duration::from_nanos(nanos));
        Self { spawn, written: HashSet::new(), sink }
    }

    /// Writes the line of `milestone`, unless the trace is off or wrote it before.
    pub fn mark(&mut self, milestone: &'static str) {
        let Some(spawn) = self.spawn else {
            return;
        };
        if self.written.insert(milestone) {
            let ms = SystemTime::now().duration_since(spawn).unwrap_or_default().as_millis();
            writeln!(self.sink, "relics: startup {milestone} {ms}").ok();
        }
    }
}

/// The trace of this process, from `RELICS_STARTUP_TRACE`. `None` when the trace is off.
static TRACE: LazyLock<Option<Mutex<Trace<Stderr>>>> = LazyLock::new(|| {
    let trace = Trace::new(std::env::var("RELICS_STARTUP_TRACE").ok().as_deref(), std::io::stderr());
    trace.spawn.is_some().then(|| Mutex::new(trace))
});

/// Writes the line of `milestone` once per launch, when the trace is on.
pub fn mark(milestone: &'static str) {
    if let Some(trace) = TRACE.as_ref() {
        trace.lock().unwrap().mark(milestone);
    }
}

/// The trace is on.
pub fn on() -> bool {
    TRACE.is_some()
}
