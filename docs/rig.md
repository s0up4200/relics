# Live-run rig

The rig runs the real Library window against the fake Silo server of the tests. An agent uses it to change something on the Server and to take a screenshot of the result. The rig is the cargo example `rig` (`examples/rig.rs`).

## What the rig does

- The app sends its API requests to the in-memory fake in `tests/common`. No API request leaves the process.
- The fake answers each request with the first successful fixture of that method and path in `tests/fixtures/silo/index.json`. An unknown request gets a 404.
- The fake keeps the Server settings of the Profile `p-owner` in memory. At start, the fake stores no value, so each setting shows its default.
- The fake has one row for each setting. It ignores `scope`, so each value comes back as a value of the Profile, also a value that the app writes for this device.
- A listener on `127.0.0.1` serves the events socket of Live updates (ADR 0006). It also takes the same API calls from `curl`.
- The window starts signed in, through `LibraryWindow::with_session`. Thus the rig uses no keyring, no sign-in screen, no video surface, and no release check. Video does not play.
- All app state goes to the directory that you give. The rig does not read or write the real Relics state, Local settings, artwork cache, or keyring items.
- The window has the app id `relics-rig`. A window rule for the rig does not move the installed Relics.

## Start the rig

Read the dotfiles `agents/docs/desktop.md` before you start. Then:

1. Build the rig. Run only one cargo command at a time.

   ```bash
   CARGO_TARGET_DIR=$HOME/.cache/relics-target nice -n 19 cargo build --example rig -j 4
   ```

2. Make sure that `hyprctl monitors all` lists `BENCH`. If it does not, run `hyprctl output create headless BENCH`. Then add the window rule:

   ```bash
   hyprctl eval 'agent_rule_relics_rig = hl.window_rule({ name = "agent-relics-rig", match = { class = "^(relics-rig)$" }, workspace = "name:bench silent" })'
   ```

3. Start the rig with a new, empty scratch directory. `$S` is your scratchpad.

   ```bash
   mkdir -p $S/rig-state
   setsid nohup $HOME/.cache/relics-target/debug/examples/rig $S/rig-state > $S/rig.log 2>&1 &
   ```

4. Wait for the line `rig: events socket open` in `$S/rig.log`. The first line gives the address, for example `rig: fake Silo at http://127.0.0.1:40505/`. Each later line is one request that the fake got, or one event that the rig sent.

## Change the Server

`$R` is the address from the first log line, with the slash at the end.

Store a Server setting. The app writes through the same route.

```bash
curl -sS -X PUT "${R}api/v2/settings/values/playback.preferred_quality?scope=profile" -d '{"value":"1080p"}'
```

Remove a Server setting:

```bash
curl -sS -X DELETE "${R}api/v2/settings/values/playback.preferred_quality?scope=profile"
```

Send one frame to each open events socket. The rig does not send an event after a write, so send it yourself:

```bash
curl -sS -X POST "${R}rig/event" -d '{"type":"event","channel":"user_settings","event":"user_settings.changed","data":{"key":"playback.preferred_quality","scope":"profile","profile_id":"p-owner"}}'
```

The reply gives the number of sockets that got the frame, for example `{"sockets":1}`. If the number is 0, the app has no open socket. Then read the log.

To serve other data, for example a new setting or a catalog change, add a route in `Fake::route` of `examples/rig.rs`.

## Move in the app and take a screenshot

Send each key to the window by its class, a `down` call and then an `up` call. The focus of the user does not change.

```bash
for state in down up; do
  hyprctl eval "hl.dispatch(hl.dsp.send_key_state({ mods = '', key = 'comma', state = '$state', window = 'class:^(relics-rig)\$' }))"
done
```

The `,` key (`comma`) opens the Settings page, and Escape goes back. The other keys are the keys of the app.

Take each screenshot of the window, not of the full monitor, because the monitor also shows the bar of the desktop. `hyprctl clients -j` gives `at` and `size` of the window with the class `relics-rig`:

```bash
grim -g "<x>,<y> <width>x<height>" $S/before.png
```

Use the same rectangle for the before and the after image. The window shows only fake data and the address `127.0.0.1`, so a capture has no token and no real server address.

## Clean up

Clean up in this order, also when the run fails:

1. Stop the rig. The command line must match the start command:

   ```bash
   pkill -xf "$HOME/.cache/relics-target/debug/examples/rig $S/rig-state"
   ```

2. Make sure that `hyprctl clients -j` shows no window with the class `relics-rig`.
3. Turn off the window rule:

   ```bash
   hyprctl eval 'agent_rule_relics_rig:set_enabled(false); if agent_rule_relics_rig:is_enabled() then error("rule still on") end'
   ```
