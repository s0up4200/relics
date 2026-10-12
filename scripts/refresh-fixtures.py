#!/usr/bin/env python3
"""Copy the Silo server API v2 fixtures that the app uses into tests/fixtures/silo/.

Usage: scripts/refresh-fixtures.py [SILO_SERVER_CHECKOUT]   (default: $SILO_SERVER or ~/github/oss/silo-server)

The script keeps each fixture of an operation in api/operations.txt, and each shared
Problem fixture that has no operation. It writes the server commit to SERVER_COMMIT.
Run it by hand, then run scripts/gen-api-types.py from the same server commit.
"""

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SERVER = Path(sys.argv[1] if len(sys.argv) > 1 else os.environ.get("SILO_SERVER", "~/github/oss/silo-server")).expanduser()
SRC = SERVER / "contracts/api/v2/fixtures"
DST = REPO / "tests/fixtures/silo"


def main():
    commit = subprocess.check_output(["git", "-C", str(SERVER), "rev-parse", "HEAD"], text=True).strip()
    wanted = {line.strip() for line in (REPO / "api/operations.txt").read_text().splitlines()
              if line.strip() and not line.startswith("#")}
    index = json.loads((SRC / "index.json").read_text())
    keep = [f for f in index["fixtures"]
            if f["operation_id"] in wanted
            or (f["operation_id"] is None and f["schema"] == "#/components/schemas/Problem")]

    shutil.rmtree(DST, ignore_errors=True)
    DST.mkdir(parents=True)
    for fixture in keep:
        if fixture.get("body_file"):
            shutil.copy(SRC / fixture["body_file"], DST / fixture["body_file"])
    (DST / "index.json").write_text(json.dumps({"fixtures": keep}, indent=2) + "\n")
    (DST / "SERVER_COMMIT").write_text(commit + "\n")
    print(f"{len(keep)} fixtures from {commit} -> {DST.relative_to(REPO)}")


main()
