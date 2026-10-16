# Research: which load an mpv event comes from

Date: 2026-10-16. Status: research only. No production code changed. Issue: [#115](https://github.com/s0up4200/relics/issues/115).

Relics keeps one mpv handle open from one Item to the next, and for each reload. The adapter (`src/mpv.rs`) turns mpv events into Playback session inputs, and these inputs name no file. This research answers one question. Can an event of an old load reach the Playback session of a later load on the same handle, and does that cause a fault?

## Versions

| Part | Version |
|---|---|
| `libmpv2` | 6.0.0, the latest release (2026-05-12) |
| `libmpv2-sys` | 4.0.1, with the headers of mpv client API 2.5 |
| mpv on this machine | 0.41.0. The mpv links below use tag `v0.41.0`. |

## Summary

Two faults are confirmed. Both come from the same gap. The old file plays on after its Playback session ends, until the next Item loads its own file. In that time, the session of the next Item gets the events of the old file.

- The next Item never plays. The user presses Play now, Next or Previous near the end of an Item. The old file reaches its end before the start reply of the next Item arrives, and the end of file ends the new session before it loads. The Player view stays on the last frame of the old Item, and no error shows.
- The next Item shows the position of the old Item. The user picks another Item while the file of the old Item still loads. The old file then loads, and the Player view shows its position for the new Item. If the first load of the new Item fails, the recovery reloads it at the position of the old Item.

Other results:

- mpv names the playlist entry of each `start-file` and `end-file` event, but libmpv2 6.0 drops that id. Property change events name no entry.
- In 100 replacements with real libmpv, no event of the old file reached the event thread after the next `loadfile` returned. The only exception was the `end-file` of the old file, which the adapter skips. In one other run, a position value of the old file reached the event thread while the next `loadfile` ran.
- The recommended fix is a lifecycle rule in the Playback session, with no change to the adapter. Until the session sends its first load, it drops each file event. It counts an end of file only after the file of its own load loaded. This removes both confirmed faults as the tests reproduce them, and keeps the handle reuse. It does not find the load of an event.
- Entry ids in the adapter are the complete fix, but they need unsafe code. They are worth the cost only if the remaining race shows up.

## How events flow today

1. The mpv event thread in `src/mpv.rs` reads each event and sends an `Input` on a channel.
2. The controller (`src/player.rs`) reads the channel on the main thread. It drops the inputs of a closed handle (`handle_id`). Each other input goes to the Playback session that is current at receipt.
3. `Controller::play` ends the old session with `Input::End`. That sends the stop report, but it does not stop or pause the old file. The new session sends its `Load` only after the start request returns.
4. The session drops `Position`, `Duration` and `Seeked` until `FileLoaded` (`loaded`). It does not gate `FileLoaded`, `EndOfFile`, `LoadFailed` or `Tracks`.

So the current guard decides the source of an event by the time of receipt. That does not show which load produced the event.

## What the mpv documentation guarantees

- `loadfile ... replace` returns before the current file stops, and the new file can start to load before the old one stops ([input.rst L543-L548](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/input.rst#L543-L548)). mpv sends `end-file` after it stops the file ([loadfile.c L2000](https://github.com/mpv-player/mpv/blob/v0.41.0/player/loadfile.c#L2000)). So the `end-file` of the old file comes after the next load call returns.
- `start-file` and `end-file` carry the `playlist_entry_id` of their file. The `end-file` id equals the id of the matching `start-file` ([client.h L1497-L1546](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h#L1497-L1546)).
- Each playlist entry has an id that is unique for the life of the handle (`playlist/N/id`, [input.rst L3283-L3287](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/input.rst#L3283-L3287)).
- mpv coalesces property changes. It returns them only when the event queue is empty, one event per changed property ([client.h L1181-L1183](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h#L1181-L1183)). A property event carries a value, not a file. The documentation gives no order between a property event and a `start-file` or `end-file`.
- When a playlist entry turns into other entries, for example a playlist, `end-file` has the reason `redirect` and names the inserted entries (`playlist_insert_id`, [client.h L1526-L1545](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h#L1526-L1545)).
- With `keep-open`, mpv pauses at the end of the file and does not end it ([options.rst L3358-L3362](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/options.rst#L3358-L3362)).

## What the mpv 0.41.0 source does, with no documented guarantee

- `loadfile` returns the `playlist_entry_id` of the new entry as its command result ([command.c L6126-L6148](https://github.com/mpv-player/mpv/blob/v0.41.0/player/command.c#L6126-L6148)). The man page does not say so. The `playlist/0/id` property gives the same id after a `replace`, because `replace` empties the playlist first.
- When mpv queues `start-file`, `end-file` or `file-loaded`, it marks each observed property as stale ([command.c L4577-L4579](https://github.com/mpv-player/mpv/blob/v0.41.0/player/command.c#L4577-L4579), [client.c L735](https://github.com/mpv-player/mpv/blob/v0.41.0/player/client.c#L735)). A stale value is a value that mpv read before the mark. mpv returns a property event only when the queue is empty ([client.c L942-L943](https://github.com/mpv-player/mpv/blob/v0.41.0/player/client.c#L942-L943)), and never for a stale value ([client.c L1801](https://github.com/mpv-player/mpv/blob/v0.41.0/player/client.c#L1801)). So when the client gets a property event after the `end-file` of a file, mpv read that value after the file ended.
- A file that ends with `keep-open` and then gets replaced ends with the reason `stop`, not `eof` ([loadfile.c L1997](https://github.com/mpv-player/mpv/blob/v0.41.0/player/loadfile.c#L1997)).

## What libmpv2 6.0 gives the adapter

`Event::StartFile` has no data, and `Event::EndFile` has only the reason. An `end-file` with an error becomes `Err`, with no reason and no id ([events.rs L235-L242](https://docs.rs/crate/libmpv2/6.0.0/source/src/mpv/events.rs#235)). `Mpv::ctx` is public, so the adapter can read the raw `mpv_event` through the C API. The new tests in `tests/mpv.rs` do that with an `extern "C"` declaration of `mpv_wait_event`.

## What real libmpv does

The tests in `tests/mpv.rs` run real libmpv (`vo=null ao=null`) against the local HTTP stub. They read the raw events with their entry ids.

| Test | Observation |
|---|---|
| `the_end_file_of_the_old_load_comes_after_the_next_load_returns_and_names_its_entry` | The test replaces a file with another file, as a new Item or a version switch does. Then it loads the same file again at a position, as a reload does. In both cases, the `end-file` of the old entry (reason `stop`) reaches the event thread after the next `loadfile` returns, and before the `start-file` of the new entry. The recorder logs each event with the time that it reached the event thread. |
| `a_file_at_its_end_reports_eof_reached_once_and_ends_with_stop_at_the_next_load` | A file at its end reports `eof-reached=yes` once, before the next load. Its `end-file` has the reason `stop`, so the adapter makes one `EndOfFile` for it. |
| `no_value_of_the_old_file_arrives_after_the_next_load_returns_and_no_end_before_the_file_loads` | 10 replacements before, at and after the end of the old file. No value of the old file arrives after the next `loadfile` returns. No `eof-reached=yes` arrives between the `start-file` and the `file-loaded` of an entry. Ten runs (100 replacements) passed. |
| `an_hls_load_that_cannot_open_ends_three_more_entries_with_an_error` | The test loads an HLS manifest whose segments fail. The load ends as `redirect`, and then three new entries (one per segment) end with an error. The adapter sends three `LoadFailed` for one load. |

A probe that was not kept loaded two files back to back. In 5 of 5 runs, the first entry got no `start-file` and no `end-file`. So a count of `start-file` events cannot match events to loads. Only the entry id can.

A second probe measured the end of file against the media time, with null outputs. In 8 of 8 runs, `eof-reached` arrived about 40 ms before the media end. The Up Next countdown runs 10 s of wall time, and the app checks it each second. So in smooth playback the countdown ends after the end of file, and the normal end path runs. The countdown ends first only when wall time runs ahead of media time, for example after a stall. The two probes are not kept as tests, because their results depend on timing.

## Confirmed faults

Both reproductions are in `tests/controller.rs`. They use the fake media and the fake Silo server. They are `#[ignore]` until a fix has its own spec. Run them with `cargo test --test controller -- --ignored`.

### The end of the old file ends the next Item before it loads

`the_end_of_the_old_file_does_not_end_the_next_item_before_it_loads`

1. Episode 1 plays near its end. The user presses Play now, Next or Previous, or switches the version.
2. The session of episode 2 starts and waits for the start reply. Episode 1 plays on.
3. Episode 1 reaches its end. Its `EndOfFile` goes to the session of episode 2. That session has no plan and no position, so the end rule sees an Early end, and the recovery finds no plan. The session ends with no command.
4. The start reply arrives, and the ended session ignores it.

The user sees the last frame of episode 1, paused, and episode 2 never loads. No error shows, and the Player view stays open until Back. The Server gets no stop report for the session of episode 2.

The fault needs the old file to end within one start request of the user action. A start request that takes seconds, for example for a transcode decision, makes the window larger.

### The old file sets the position of the next Item

`the_file_of_the_old_item_does_not_set_the_position_of_the_next_item`

1. The user picks a movie, and then picks an episode while the movie file still loads.
2. The movie file loads. `FileLoaded`, `Duration` and `Position` go to the session of the episode, and the session takes them as its own.
3. The Player view shows the position of the movie for the episode. `load_now` does not clear the position, so it stays after the episode loads, until the episode file reports a position.
4. If the first load of the episode fails, the recovery reloads the episode at the position of the movie (300 s), not at its own start (12.5 s).

This fault needs no timing accident. Two fast clicks on Next, or a pick while a slow HLS start still loads, are enough.

## Suspected races, not reproduced

- An event in the channel when the next load starts. The adapter can send an event of the old file just before the main thread runs the start reply of the next Item. The main thread then sends the `Load`. Only after that does it read the old event. A late `EndOfFile` then starts the recovery of the new session. The result is one extra token refresh and one reload. The recovery chain also moves to `Reloaded`, so the next real failure skips the refresh and asks for a replan. The window starts when the start reply arrives, and it ends when the main thread runs the task of that reply. No test reproduces it.
- Three `LoadFailed` from one HLS load. The first starts the recovery. The other two do nothing while the refresh runs. If the refresh returned between them, a late one would move the chain to a replan. With a real server, the refresh takes longer than the segment failures, so this is unlikely.
- A property value that mpv reads just before the next `loadfile`. The source lets it arrive after the call returns, because mpv reads the value before it processes the command. The tests did not see that in 100 replacements. In an earlier version of the recorder, one `time-pos` value of the old file reached the event thread while the call ran. In the app, the controller reads such an event only after the `Load` returns, so it reaches the new session. A late `Position` does no harm, because the session drops it until `FileLoaded`. A late `EndOfFile` in the same place is the first suspected race.

## Which events can name their load

| Input | Source event | Can it name its load? |
|---|---|---|
| `LoadFailed` from `end-file` | `end-file` | Yes, by `playlist_entry_id` in the raw event. A `redirect` passes the load on to the inserted entries. |
| `FileLoaded` | `file-loaded` | Yes, by its place in the queue after the `start-file` of its entry. The source guarantees the order. The documentation does not. |
| `Seeked` | `playback-restart`, then a read of `time-pos` at receipt | The event can, as `FileLoaded` can. The position cannot, because the adapter reads it at receipt. |
| `Position`, `Duration`, `Tracks`, `EndOfFile` from `eof-reached` | property change | Only through the stale rule of the mpv source. A value after the `end-file` of a file is not from that file. The documentation does not guarantee it. |
| `Paused` | property change | Not needed. The pause state belongs to the handle, not to a file. |
| `LoadFailed` from another libmpv2 error | any `Err` | No. The adapter makes `LoadFailed` from each libmpv2 error, also from one that is not a file error. |

## Recommendation

The smallest fix that keeps the handle reuse and needs no adapter change is a lifecycle rule in the Playback session. It does not find the source of an event. It uses two facts of the session itself:

1. Before the session sends its first `Load`, no event can come from its own file. The session drops `FileLoaded`, `Position`, `Duration`, `Seeked`, `Tracks`, `EndOfFile` and `LoadFailed` until then. This removes both confirmed faults as the tests reproduce them.
2. mpv reports the end of a file only after the file loaded. The session counts `EndOfFile` only after the `FileLoaded` of its current load, the rule that `Position` already follows. This closes the late `EndOfFile` of the first suspected race, unless a late `FileLoaded` also slips through.

These rules still decide by the time that the controller reads an event. An event of the old file that the controller reads after the `Load` of the session still reaches the session. Only entry ids close that gap.

No current behavior changes with these rules. In the tests, mpv sends no `eof-reached=yes` before `file-loaded` of an entry. A failed open arrives as `LoadFailed`, which rule 2 does not gate.

The complete fix attributes events by entry id. Do it only if the remaining race shows up in use:

- The adapter reads the raw `mpv_event` through `Mpv::ctx` and the C API, to get the entry id of `start-file` and `end-file`. That needs unsafe code, and `libmpv2-sys` (already in `Cargo.lock`) as a direct dependency, or a change to libmpv2.
- `Handle::load` returns the entry id of the load. Use the result of the `loadfile` command, not `playlist/0/id`, because a playlist entry can redirect before the property read.
- The event thread tags each input with the entry of the last `start-file`, and clears the tag at `end-file`. On a `redirect`, the inserted entries take the load of the entry that ended.
- Property values follow the stale rule of the mpv source, so this fix depends on behavior that the mpv documentation does not promise. Keep `no_value_of_the_old_file_arrives_after_the_next_load_returns_and_no_end_before_the_file_loads` to catch a change in mpv.
- The controller drops each input whose entry is not the entry of the latest `Load`.

## Limits

- The tests use `vo=null ao=null`. The Player view uses `vo=libmpv` and a real audio output, which can change the timing of the end of file.
- 100 replacements on one machine show no value after the call returns. They do not prove that none can arrive.
- No test drives the controller with real libmpv, because `InView` needs a Wayland surface. The confirmed faults combine a real mpv fact (the old file plays on and reports its end) with a controller reproduction that uses the fake media.
- All requests went to the local HTTP stub or the fake Silo server.

## Other observations

- `a_401_in_the_middle_of_an_hls_stream_ends_it_before_the_duration` does not test a failure in the middle of the stream. With `stub(2)`, both answered requests are for the manifest, and the first segment already gets a 401. So the test covers a failed open, which sends three `LoadFailed`.
- With `stub(3)`, the first segment plays and then each request gets a 401. With `vo=null`, mpv then seeks to the last frame at the end of file. The adapter sends `Position(6.0)` before `EndOfFile`, and the end rule sees a Normal end, not an Early end. This was not tested with `vo=libmpv`. If it also happens there, an expired token in the middle of an HLS stream marks the Item watched and skips the recovery. That question needs its own issue.
