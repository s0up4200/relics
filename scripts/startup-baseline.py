#!/usr/bin/env python3
"""Measure the startup time of the release build against a Server.

Usage: scripts/startup-baseline.py SERVER_HOST [--cold]

The script builds the plain release build, then launches it with RELICS_STARTUP_TRACE set to the spawn time.
It reads only the `relics: startup <milestone> <ms>` lines from stderr, and stops the app after `first-poster`,
or after 30 seconds. A warm series is 1 warm-up launch that does not count, then 10 measured launches. With
--cold, the script empties the OS file cache before each of 5 launches. This needs sudo.

Before the series, `tailscale ping` gives the round-trip time and the path (direct or DERP) to SERVER_HOST.
The report gives the median and the p90 of each milestone, and the launches where a milestone did not come.
It contains no Server address and no tailnet name, so it can go in a public issue. Quit Relics before a run.
"""

import argparse
import math
import os
import queue
import re
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MILESTONES = [
    "main",
    "first-frame",
    "keyring",
    "profiles",
    "settings",
    "libraries",
    "home-layout",
    "home-sections",
    "home-refetch",
    "home-ready",
    "first-poster",
]
LINE = re.compile(r"^relics: startup (\S+) (\d+)$")
TIMEOUT = 30


def build():
    """The path of the release build, built with the same profile as the AUR package."""
    # The shared target dir of the repo. A second target dir is a cold build of about 16 GB.
    target = Path(os.environ.get("CARGO_TARGET_DIR", Path.home() / ".cache/relics-target"))
    command = ["nice", "-n", "19", "cargo", "build", "--release", "-j", "4"]
    subprocess.run(command, cwd=REPO, env=dict(os.environ, CARGO_TARGET_DIR=str(target)), check=True)
    return target / "release" / "relics"


def ping(host):
    """The round-trip time and the path of the last pong. The output of tailscale names the host, so it stays hidden."""
    result = subprocess.run(["tailscale", "ping", host], capture_output=True, text=True)
    pongs = re.findall(r"via (\S+) in (\S+)", result.stdout)
    if not pongs:
        return "no pong", "unknown"
    via, rtt = pongs[-1]
    # A direct path names an IP address and a port. Only a DERP region is safe to print.
    path = via if via.startswith("DERP(") else "peer relay" if via.startswith("peer-relay") else "direct"
    return rtt, path


def drop_caches():
    subprocess.run(["sudo", "sh", "-c", "sync && echo 3 > /proc/sys/vm/drop_caches"], check=True)


def launch(binary):
    """The milestones of one launch, as {milestone: ms}."""
    env = dict(os.environ, RELICS_STARTUP_TRACE=str(time.time_ns()))
    app = subprocess.Popen(
        [binary], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True
    )
    lines = queue.Queue()
    # None marks the end of stderr: the app quit.
    threading.Thread(target=lambda: [*map(lines.put, app.stderr), lines.put(None)], daemon=True).start()
    seen = {}
    deadline = time.monotonic() + TIMEOUT
    while "first-poster" not in seen and (left := deadline - time.monotonic()) > 0:
        try:
            line = lines.get(timeout=left)
        except queue.Empty:
            break
        if line is None:
            break
        if match := LINE.match(line.rstrip("\n")):
            seen[match[1]] = int(match[2])
    app.terminate()
    try:
        app.wait(5)
    except subprocess.TimeoutExpired:
        app.kill()
        app.wait()
    return seen


def p90(values):
    """The nearest-rank 90th percentile."""
    ordered = sorted(values)
    return ordered[math.ceil(0.9 * len(ordered)) - 1]


def report(binary, kind, runs, rtt, path):
    identity = subprocess.run([binary, "--version"], capture_output=True, text=True).stdout.strip()
    print(f"{identity}, {kind}, {len(runs)} launches, tailscale ping {rtt} via {path}")
    print()
    print("| milestone | median ms | p90 ms | missing |")
    print("|---|---:|---:|---:|")
    for milestone in MILESTONES:
        values = [run[milestone] for run in runs if milestone in run]
        missing = len(runs) - len(values)
        if values:
            print(f"| {milestone} | {statistics.median(values):.0f} | {p90(values)} | {missing} |")
        else:
            print(f"| {milestone} | - | - | {missing} |")


def main():
    parser = argparse.ArgumentParser(description="Measure the startup time of the release build.")
    parser.add_argument("host", help="the Tailscale host of the Server; the report does not show it")
    parser.add_argument("--cold", action="store_true", help="empty the OS file cache before each of 5 launches")
    args = parser.parse_args()
    if subprocess.run(["pgrep", "-x", "relics"], capture_output=True).returncode == 0:
        sys.exit("Quit Relics first: a running app takes the hand-off socket and changes the numbers.")
    binary = build()
    rtt, path = ping(args.host)
    if args.cold:
        runs = []
        for _ in range(5):
            drop_caches()
            runs.append(launch(binary))
    else:
        launch(binary)
        runs = [launch(binary) for _ in range(10)]
    report(binary, "cold" if args.cold else "warm", runs, rtt, path)


if __name__ == "__main__":
    main()
