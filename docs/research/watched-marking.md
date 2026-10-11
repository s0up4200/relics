# How Silo marks an Item watched

Ticket: #13. Map: #1.

Source: the repository `Silo-Server/silo-server` at commit `ec1dcb67c77f59688692574516276e8f6c72dfeb`. Every path below belongs to that repository at that commit. No request went to a running Silo server.

## Answer

The server marks an Item watched by itself. For a normal play, the client does not call any extra endpoint.

- Rule: a progress sample marks the Item watched when `position / duration > 0.90`. The test is strict, so exactly 90% does not count. The `duration` is the server's own file duration. It is not a value from the client. Source: `internal/userstore/threshold.go`, `ResolveProgressState` and `WatchedFraction`.
- Admin setting: the threshold is the server setting `playback.watched_threshold`, default 90. The minimum-resume floor is `playback.min_resume_threshold`, default 5. The OpenAPI file does not expose either value. The client must not copy the 90 into its own logic. It must read the result from the server.
- Minimum resume: the server discards a sample when `position / duration < 0.05`. It also ignores position 0 (`persistProgress` in `internal/api/handlers/playback.go`).
- On completion: the server sets `completed = true` and resets the stored `position_seconds` to 0. The Item then has no resume point.
- Latch: `completed` is a one-way latch for playback writes. A later heartbeat, even a low one, does not clear it. A rewatch of a completed Item stores a new resume position, and the watched flag stays. Source: `internal/userstore/pgstore/progress.go`, `UpdateProgress` and `SetProgress`.

## Calls that write progress

| Call | operationId | Effect |
| --- | --- | --- |
| `POST /api/v2/playback/{session_id}/progress` | `updatePlaybackProgress` | Body: `installation_id`, `sequence` (1 or more), `position` (seconds of source time), `is_paused`. Each applied sample goes through the threshold rule. |
| `DELETE /api/v2/playback/{session_id}` | `stopPlayback` | Body: `installation_id`, `stop_id` (canonical UUID), and an optional final sample `sequence` + `position` + `is_paused` (both or neither). Writes the final position and a watch-history row. The row has `Completed` true under the same `> 0.90` rule. |
| `POST /api/v2/sync/progress` | `syncProgress` | Batch of up to 100 items `{media_item_id, position_ms, duration_ms, updated_at?, force_overwrite?, client_ref?}`. Same rule, with the `duration_ms` of the client. For clients that keep their own timeline. |
| `POST /api/v2/watched/{id}` | `markWatched` | Manual mark. Answers 204. |
| `DELETE /api/v2/watched/{id}` | `unmarkWatched` | Manual unmark. Answers 204. |

All of these need the header `X-Profile-Id`. A PIN-locked profile also needs `X-Profile-Token`.

Sources: `contracts/api/v2/openapi.json`. `internal/api/handlers/playback_service.go` (`ApplyProgressV2`, `StopPlaybackV2`, `finishStopV2`). `internal/api/handlers/playback.go` (`persistProgress`, `persistStopAndHistory`). `internal/api/handlers/progress.go` (sync path). `docs/playback-api.md`, section "Progress and stop".

## Rules for a progress sample

- The server orders samples by `sequence` with a compare-and-set. A higher sequence wins, even when `position` moves backward.
- A lower sequence gives 200 `outcome: "stale_sample"`. The same sequence with the same payload gives 200 `outcome: "replayed"`. The same sequence with a different payload gives 409 `progress_conflict`.
- A progress call after the stop, or for an unknown session, gives 404 `playback_session_not_found`. This means the session ended.
- The client posts progress every 10 seconds while the session is open, playing or paused. Source: `docs/architecture/playback-protocol-v3.md`.
- The position is the source position: the player position plus `timeline_offset_seconds`.

## Manual mark and unmark

- `markWatched` accepts a movie, ebook, episode, season or series. A season or series expands to its episodes. The mark is atomic across the set.
- A mark is idempotent for each Item. An Item that is already completed gets no new history row (`dropCompletedTargets` in `internal/watchstate/service.go`).
- `markWatched` writes progress with `completed = true`, `position_seconds = 0` and the real duration (`MarkWatched` in `pgstore/progress.go`).
- `unmarkWatched` is the undo call for a mark that the client set by mistake. It also works on a season or series.
- Unmark has a side effect. It hides the history of the Item up to now and deletes the whole progress row (`removeHistoryItems` in `pgstore/progress.go`). The resume position is lost. The SQL does not filter by source, so unmark also clears a watched state that came from playback.
- A client that wants to keep the resume point must remember the position and send it again with `syncProgress` and `force_overwrite: true`.
- Only manual plays produce an outbound provider event on unmark (`RecordManualMarkUnwatchedWithResult` passes `WatchHistorySourceManual`).

## Early end of file

The server has no special case for an early end of file. It treats the last position as a normal sample.

- At 90% or less and at least 5%, the Item stays in progress. The resume point is that position.
- Below 5%, the server discards the sample. The old resume point stays.
- Above 90%, the Item becomes watched.
- Position 0 writes nothing. A zero sample after an early end of file does not wipe the resume point.
- A stop with a final sample follows the same rules. A stop with no final sample uses the last accepted progress sample. If that position is 0, the stop writes no history row.
- After the stop, every progress call gives 404, and every repeated `DELETE` replays the stored receipt (`outcome: "replayed"`). The client must send its last position before the stop or inside it.

## Client persistence mode

The start request has the field `progress_persistence`. When it is omitted or `server`, the server writes resume and history from progress and stop. When it is `client`, the server writes neither. The client then must send `start_position` and write its own timeline with `syncProgress`. The audiobook flow uses `client`. A video client does not need it.

## Open points

- The progress and stop responses do not carry a `completed` flag. `PlaybackMutation` has `outcome`, `accepted`, `history_id` and `stop_id`. The client learns the watched state from `listProgress` (`GET /api/v2/progress`, schema `ProgressEntry`, field `completed`) or from a `watched` event. `history_id` appears only when the stop wrote a history row.
- An admin can change both threshold settings, and the API does not report them.
- This research did not read the full field list of `ProgressEntry`. Read `components.schemas.ProgressEntry` before type generation.
