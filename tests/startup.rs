use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relics::startup::Trace;

/// The lines that a trace from `value` writes for `milestones`.
fn written(value: Option<&str>, milestones: &[&'static str]) -> String {
    let mut sink = Vec::new();
    let mut trace = Trace::new(value, &mut sink);
    for milestone in milestones {
        trace.mark(milestone);
    }
    String::from_utf8(sink).unwrap()
}

/// The env var value of a spawn time `ago` in the past.
fn spawned(ago: Duration) -> String {
    (SystemTime::now() - ago).duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
}

#[test]
fn a_trace_without_a_value_writes_nothing() {
    assert_eq!(written(None, &["main"]), "");
}

#[test]
fn a_trace_with_a_value_that_is_not_a_number_writes_nothing() {
    assert_eq!(written(Some("yes"), &["main"]), "");
    assert_eq!(written(Some(""), &["main"]), "");
    assert_eq!(written(Some("-5"), &["main"]), "");
}

#[test]
fn a_milestone_line_gives_the_milliseconds_since_the_spawn_time() {
    let lines = written(Some(&spawned(Duration::from_millis(1500))), &["home-ready"]);
    let ms: u64 =
        lines.strip_prefix("relics: startup home-ready ").unwrap().strip_suffix('\n').unwrap().parse().unwrap();
    assert!((1500..2000).contains(&ms), "{ms}");
}

#[test]
fn a_second_mark_of_the_same_milestone_writes_nothing() {
    let lines = written(Some(&spawned(Duration::ZERO)), &["main", "first-frame", "main"]);
    let milestones: Vec<_> = lines.lines().map(|line| line.split(' ').nth(2).unwrap()).collect();
    assert_eq!(milestones, ["main", "first-frame"]);
}
