#!/usr/bin/env python3
"""Writes contributors/contributors.json from the pull requests opened on this repository only.

Upstream's contributors are not listed one by one; the About window credits them in one line.
A person is listed when a pull request of theirs here was merged, or was landed for them by a
maintainer's `land/<n>` or `bundle/<a>-<b>` pull request (whose body names it as `#n`). The
landing pull request itself counts for nobody, so the work is credited once, to its author.
Model credits come from the `Co-Authored-By:` lines of this repository's own commits (not
upstream's).

Usage: python3 contributors/fork-contributors.py   (needs `gh` signed in, and the `upstream`
remote fetched, run from the repository root)
"""
import json
import pathlib
import re
import subprocess
import tomllib

REPO = "emmertex/lightcraft-classic"
BOTS = {"dependabot", "github-actions", "renovate"}
FIELDS = "number,author,state,headRefName,body,additions,deletions,createdAt,mergedAt,closedAt"


def gh(*args):
    return json.loads(subprocess.run(["gh", *args], check=True, capture_output=True, text=True, encoding="utf-8").stdout)


def git(*args):
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True, encoding="utf-8").stdout


def main():
    root = pathlib.Path(__file__).resolve().parent.parent
    prs = gh("pr", "list", "--repo", REPO, "--state", "all", "--limit", "5000", "--json", FIELDS)
    landing = [p for p in prs if p["state"] == "MERGED" and re.match(r"(land|bundle)/", p["headRefName"])]
    landed = {int(n) for p in landing for n in re.findall(r"#(\d+)", p["body"] or "")}
    landing_numbers = {p["number"] for p in landing}

    people = {}
    for p in prs:
        login = (p.get("author") or {}).get("login") or ""
        if not login or login.removesuffix("[bot]") in BOTS or p["number"] in landing_numbers:
            continue
        if not (p["state"] == "MERGED" or (p["state"] == "CLOSED" and p["number"] in landed)):
            continue
        c = people.setdefault(login, {"login": login, "prs": 0, "commits": 0, "lines_added": 0, "lines_deleted": 0, "dates": []})
        c["prs"] += 1
        # (the commit count per pull request: the list query can't carry it, GitHub's node limit)
        c["commits"] += int(subprocess.run(["gh", "api", f"repos/{REPO}/pulls/{p['number']}", "--jq", ".commits"], check=True, capture_output=True, text=True).stdout.strip() or 0)
        c["lines_added"] += p["additions"]
        c["lines_deleted"] += p["deletions"]
        c["dates"] += [d for d in (p["createdAt"], p["mergedAt"] or p["closedAt"]) if d]

    with open(root / "contributors" / "people.toml", "rb") as f:
        entries = tomllib.load(f).get("people", {})
    rows = []
    for c in sorted(people.values(), key=lambda c: (-c["prs"], c["login"].lower())):
        dates = sorted(c.pop("dates"))
        rows.append({
            "login": c["login"],
            "people_entry": c["login"] in entries,
            "github_name": None,
            "prs": c["prs"],
            "commits": c["commits"],
            "lines_added": c["lines_added"],
            "lines_deleted": c["lines_deleted"],
            "binary_added": 0,
            "binary_deleted": 0,
            "first_commit": dates[0] if dates else "",
            "last_commit": dates[-1] if dates else "",
        })

    # model credits: this repository's own commits (on main, not reachable from upstream)
    log = git("-C", str(root), "log", "--no-merges", "--format=%H%x00%B%x01", "upstream/main..origin/main")
    models = {}
    for entry in log.split("\x01"):
        body = entry.partition("\x00")[2]
        seen = set()
        for name in re.findall(r"(?im)^co-authored-by:\s*(Claude[^<]*?)\s*<[^>]*anthropic\.com>", body):
            m = re.match(r"Claude(?:\s+(\w+))?(?:\s+([\d.]+))?", name.strip())
            family, version = (m.group(1) or "", m.group(2) or "") if m else ("", "")
            key = (f"Claude {family}".strip(), version)
            if key not in seen:
                seen.add(key)
                models[key] = models.get(key, 0) + 1
    commits = sum(1 for e in log.split("\x01") if e.strip())
    model_rows = [
        {"id": f"anthropic/{m.lower().replace(' ', '-')}/{v}".rstrip("/"), "company": "Anthropic", "model": m, "version": v,
         "commits": n, "lines_added": 0, "lines_deleted": 0}
        for (m, v), n in sorted(models.items(), key=lambda kv: -kv[1])
    ]

    head = git("-C", str(root), "rev-parse", "origin/main").strip()
    out = {
        "schema": 1,
        "repo": REPO,
        "branch": "main",
        "head": head,
        "head_date": git("-C", str(root), "show", "-s", "--format=%cI", head).strip(),
        "commits": commits,
        "unattributed_commits": 0,
        "contributors": rows,
        "models": model_rows,
    }
    (root / "contributors" / "contributors.json").write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(rows)} contributors, {len(model_rows)} models, {commits} fork commits")


if __name__ == "__main__":
    main()
