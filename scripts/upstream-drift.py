#!/usr/bin/env python3

import argparse
import json
import os
import urllib.request
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
UPSTREAM = "Silo-Server/silo-server"
SPEC = "contracts/api/v2/openapi.json"
PREFIX = "#/components/schemas/"
WORDING = {"description", "summary", "title", "example", "examples", "externalDocs"}
RULES = {"enum", "format", "pattern", "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum",
         "minLength", "maxLength", "minItems", "maxItems", "uniqueItems"}
NAMES = {"properties", "patternProperties", "$defs", "dependentSchemas", "dependentRequired",
         "headers", "content", "encoding", "responses", "links", "callbacks", "mapping"}
REQUEST, RESPONSE = "request", "response"
HELP = """Report how the Silo API v2 changed since the server commit that Relics pins
in tests/fixtures/silo/SERVER_COMMIT. Compares the operations in api/operations.txt
and the schemas that they reach. Prints Markdown, or nothing when the API that Relics
uses did not change and Silo has no new operation outside the admin API.
Set GH_TOKEN for a higher GitHub rate limit."""


def get(url, accept="application/vnd.github+json"):
    headers = {"Accept": accept, "User-Agent": "relics-upstream-drift"}
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as resp:
        return json.load(resp)


def commit(ref):
    data = get(f"https://api.github.com/repos/{UPSTREAM}/commits/{ref}")
    return data["sha"], data["commit"]["committer"]["date"][:10]


def spec(sha):
    return get(f"https://api.github.com/repos/{UPSTREAM}/contents/{SPEC}?ref={sha}", "application/vnd.github.raw")


def refs(node):
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str) and ref.startswith(PREFIX):
            yield ref[len(PREFIX):]
        for value in node.values():
            yield from refs(value)
    elif isinstance(node, list):
        for value in node:
            yield from refs(value)


def operations(doc):
    out = {}
    for path, item in doc["paths"].items():
        for method, op in item.items():
            if isinstance(op, dict) and "operationId" in op:
                out[op["operationId"]] = (f"{method.upper()} {path}", op)
    return out


def reach(doc, node):
    schemas, keep, stack = doc["components"]["schemas"], set(), list(refs(node))
    while stack:
        name = stack.pop()
        if name not in keep and name in schemas:
            keep.add(name)
            stack.extend(refs(schemas[name]))
    return keep


def sides(doc, op):
    out = defaultdict(set)
    for name in reach(doc, [op.get("parameters"), op.get("requestBody")]):
        out[name].add(REQUEST)
    for name in reach(doc, op.get("responses")):
        out[name].add(RESPONSE)
    return out


def strip(node, keyed_by_name=False):
    if isinstance(node, dict):
        if keyed_by_name:
            return {k: strip(v) for k, v in node.items()}
        return {k: strip(v, k in NAMES) for k, v in node.items() if k not in WORDING and not k.startswith("x-")}
    if isinstance(node, list):
        return [strip(v) for v in node]
    return node


def by_name(params):
    return {f"{p.get('in')} {p.get('name')}": p for p in params or [] if isinstance(p, dict)}


def names(values):
    return {json.dumps(v) if not isinstance(v, str) else v for v in values or []}


def diff(old, new, path=()):
    if isinstance(old, dict) and isinstance(new, dict):
        for key in sorted(old.keys() | new.keys(), key=str):
            a, b = old.get(key), new.get(key)
            if a == b:
                continue
            if key == "properties" and isinstance(a or {}, dict) and isinstance(b or {}, dict):
                a, b = a or {}, b or {}
                for name in sorted(b.keys() - a.keys()):
                    yield "new field", path + (name,), ""
                for name in sorted(a.keys() - b.keys()):
                    yield "removed field", path + (name,), ""
                for name in sorted(a.keys() & b.keys()):
                    yield from diff(a[name], b[name], path + (name,))
            elif key in RULES and a is None:
                yield "new rule", path, f"{key} {json.dumps(b)}"
            elif key in ("required", "enum") and isinstance(a or [], list) and isinstance(b or [], list):
                added, gone = sorted(names(b) - names(a)), sorted(names(a) - names(b))
                more, less = ("now required", "now optional") if key == "required" else ("new value", "removed value")
                if added:
                    yield more, path, ", ".join(added)
                if gone:
                    yield less, path, ", ".join(gone)
            elif key == "parameters":
                a, b = by_name(a), by_name(b)
                for name in sorted(b.keys() - a.keys()):
                    yield ("new required parameter" if b[name].get("required") else "new parameter"), (name,), ""
                for name in sorted(a.keys() - b.keys()):
                    yield "removed parameter", (name,), ""
                for name in sorted(a.keys() & b.keys()):
                    yield from diff(a[name], b[name], (f"parameter {name}",))
            elif key == "responses" and isinstance(a or {}, dict) and isinstance(b or {}, dict):
                a, b = a or {}, b or {}
                for code in sorted(b.keys() - a.keys()):
                    yield "new response", (code,), ""
                for code in sorted(a.keys() - b.keys()):
                    yield "removed response", (code,), ""
                for code in sorted(a.keys() & b.keys()):
                    yield from diff(a[code], b[code], (f"response {code}",))
            elif isinstance(a, (dict, list)) and isinstance(b, (dict, list)):
                yield from diff(a, b, path + (str(key),))
            elif key == "$ref" and a and b:
                yield "changed type", path, (a.removeprefix(PREFIX), b.removeprefix(PREFIX))
            elif key == "deprecated":
                yield "deprecated", path, ""
            else:
                yield "changed", path, f"{key}: {json.dumps(a)} → {json.dumps(b)}"
    elif isinstance(old, list) and isinstance(new, list) and len(old) == len(new):
        for i, (a, b) in enumerate(zip(old, new)):
            yield from diff(a, b, path + (str(i),))
    elif old != new:
        yield "changed", path, f"{json.dumps(old)} → {json.dumps(new)}"


def op_side(path):
    if path and path[0].startswith("response "):
        return {RESPONSE}
    return {REQUEST}


def grade(kind, where):
    if kind in ("new field", "new value", "new parameter", "new response", "deprecated"):
        return "new"
    if kind == "new rule":
        return "check" if REQUEST in where else None
    if kind in ("now required", "removed value"):
        return "check" if REQUEST in where else "new"
    if kind == "now optional":
        return "check" if RESPONSE in where else "new"
    return "check"


def bullet(kind, path, detail):
    if isinstance(detail, tuple):
        detail = " → ".join(detail)
    where = ".".join(path)
    text = f"{where}: {detail}" if where and detail else where or detail
    return f"- {kind}: {text}" if text else f"- {kind}"


def is_admin(route, op):
    return route.split(" ", 1)[1].startswith("/api/v2/admin") or any(t.startswith("admin") for t in op.get("tags") or [])


def main():
    parser = argparse.ArgumentParser(description=HELP, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("ref", nargs="?", default="main", help="Silo ref to compare with (default: main)")
    args = parser.parse_args()
    pin = (REPO / "tests/fixtures/silo/SERVER_COMMIT").read_text().strip()
    wanted = sorted({line.strip() for line in (REPO / "api/operations.txt").read_text().splitlines()
                     if line.strip() and not line.startswith("#")})
    pin_sha, pin_date = commit(pin)
    head_sha, head_date = commit(args.ref)
    old, new = spec(pin_sha), spec(head_sha)
    old_ops, new_ops = operations(old), operations(new)
    old_schemas, new_schemas = old["components"]["schemas"], new["components"]["schemas"]

    found = {"check": defaultdict(list), "new": defaultdict(list)}
    quiet = 0
    users, where = defaultdict(set), defaultdict(set)
    gone = [w for w in wanted if w in old_ops and w not in new_ops]

    def record(key, changes, side_of):
        nonlocal quiet
        for kind, path, detail in list(changes):
            if kind == "changed type" and detail[0] in old_schemas and detail[1] in new_schemas:
                changes.extend(diff(strip(old_schemas[detail[0]]), strip(new_schemas[detail[1]]), path))
        graded = [(grade(kind, side_of(path)), bullet(kind, path, detail)) for kind, path, detail in changes]
        graded = [(level, text) for level, text in graded if level]
        if not graded:
            quiet += 1
        for level, text in graded:
            found[level][key].append(text)

    for w in wanted:
        if w not in old_ops or w not in new_ops:
            continue
        for doc, (_, op) in ((old, old_ops[w]), (new, new_ops[w])):
            for name, side in sides(doc, op).items():
                users[name].add(w)
                where[name] |= side
        (old_route, old_op), (new_route, new_op) = old_ops[w], new_ops[w]
        key = f"`{w}` {new_route}"
        if old_route != new_route:
            found["check"][key].append(f"- moved: {old_route} → {new_route}")
        if old_op != new_op:
            record(key, list(diff(strip(old_op), strip(new_op))), op_side)
    for name in sorted(users):
        if name not in old_schemas or name not in new_schemas or old_schemas[name] == new_schemas[name]:
            continue
        side = " and ".join(sorted(where[name]))
        key = f"`{name}`, {side} of {', '.join(sorted(users[name]))}"
        record(key, list(diff(strip(old_schemas[name]), strip(new_schemas[name]))), lambda _: where[name])

    unused = defaultdict(list)
    for op_id in sorted(new_ops.keys() - old_ops.keys()):
        route, op = new_ops[op_id]
        if not is_admin(route, op):
            unused[(op.get("tags") or ["other"])[0]].append(f"- `{op_id}` {route}")

    if not (gone or found["check"] or found["new"] or unused):
        return

    out = [
        f"Relics pins Silo at [`{pin_sha[:7]}`](https://github.com/{UPSTREAM}/commit/{pin_sha}) ({pin_date}). "
        f"Silo `{args.ref}` is at [`{head_sha[:7]}`](https://github.com/{UPSTREAM}/commit/{head_sha}) ({head_date}). "
        f"[All changes](https://github.com/{UPSTREAM}/compare/{pin_sha[:12]}...{head_sha[:12]}).",
        "",
        "To update the pin, follow `docs/silo-upstream.md`. When nothing is left, this issue closes by itself.",
        "",
    ]

    def section(title, intro, groups):
        out.extend([f"## {title} ({sum(len(v) for v in groups.values())})", "", intro, ""])
        for key in sorted(groups):
            out.extend([f"<details><summary>{key}: {len(groups[key])}</summary>", ""]
                       + groups[key] + ["", "</details>", ""])

    if gone:
        out.extend(["## Calls that Silo removed", ""] + [f"- `{w}`" for w in gone] + [""])
    section("Changes to check", "Each one can break a request that Relics sends, or a response that Relics reads.",
            found["check"])
    section("New fields and values", "Relics can ignore these. Code that matches on known values can meet a new one.",
            found["new"])
    section("New calls outside the admin API", "Relics does not use these yet.", unused)
    out.append(f"{quiet} calls and schemas changed only in descriptions, or in rules for values that Silo sends.")
    print("\n".join(out))


main()
