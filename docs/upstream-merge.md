# Upstream (LightCraft): review and port, no more merges

This repository is a fork of LightCraft. **Since 2026-10-25 it no longer merges upstream.** The last merge
(PR #486, 281 upstream commits, 88 conflicted files) showed where the cost is: the fork's UI (the Modern layout, the
full-window Settings, Import and Export screens, the dialog cards) replaced upstream's, so every merge conflicts
there and brings in upstream UI changes, tests and translations that have to be undone or rewritten. What upstream
still offers that helps us is mostly in the engine, raw decoding, the pipeline, the GPU, the catalog and security
fixes, and those port cleanly as single commits. So upstream is now **reviewed weekly and ported by hand**: we
cherry-pick what helps and skip the rest. Merging was not merely inconvenient — 281 commits and 88 conflicts per
round meant conflict resolutions quietly brought upstream behaviour back in, which is where the unintended
degradation came from.

> **The primary path is PR intake:** [`docs/upstream-pr-intake.md`](upstream-pr-intake.md) covers open, closed and
> merged pull requests, the local ledger, and the single fork PR per round that merges only at a version release.
> This page keeps the commit-level lane (upstream commits that never had a PR), the cherry-pick mechanics, and the
> outbound lane (our fixes offered upstream). `cargo xtask upstream-merge` is retired: do not use it.

The fork point is tagged `fork-base`; the last merge is `9601cbbc6` (upstream commit `38f8d2508`). The last upstream
commit reviewed since then is recorded in [`upstream-reviewed.txt`](../upstream-reviewed.txt).

## Remotes (one-time)

```sh
git remote -v                          # upstream = the LightCraft repository, origin = this fork
git remote add upstream ../lightcraft  # only if it is missing
```

## The weekly review

1. **List what is new.**
   ```sh
   cargo xtask upstream-review          # fetches upstream; writes target/upstream/upstream-review-YYYYMMDD.md
   ```
   Every upstream commit since the last review, oldest first, with the areas it touches (`crates/raw`,
   `apps/app`, `docs`, …). Paths that are ours ([`upstream-owned.txt`](../upstream-owned.txt) → `[owned]`, which
   includes `crates/ui-egui/` and `crates/catalog/`) are marked *(ours)*, and a commit that touches only those says so.
   This is the **commit lane only**: it does not list pull requests. Enumerate those with
   `python3 tools/upstream_intake.py enumerate` and follow [`docs/upstream-pr-intake.md`](upstream-pr-intake.md).
   A commit that arrived through a PR is decided from the PR, in the ledger, not here.
2. **Decide per commit.** Port it when it helps the fork:
   - **Port:** bug fixes, security fixes, crash fixes, raw / codec / camera support, pipeline and GPU fixes,
     performance, catalog fixes (by hand: the catalog diverged), engine features we lack.
   - **Usually skip:** UI reworks of things the fork redesigned (Settings, menus, dialogs, panels the Modern layout
     replaced), label renames, upstream's own CI and packaging, its docs and What's New, features that conflict with
     the fork's behaviour (e.g. the fork's drop rule: photo files at once, a folder opens the review).
   - When unsure, ask whether a user of the fork would notice the difference. If not, skip it.
   - **Watchlist:** `WATCH` rows in [`upstream-pr-reviews/ledger.md`](upstream-pr-reviews/ledger.md) are taken without a
     fresh review once upstream approves them or merges them; they are re-checked every round.
3. **Port by cherry-pick**, one commit (or a small related group) at a time:
   ```sh
   git cherry-pick -x <hash>                            # -x records "(cherry picked from commit …)"
   cargo xtask rename-crates --upstream --since HEAD~1  # map upstream crate names (lightcraft_x → lcc_x)
   cargo xtask brand check                              # no upstream product name anywhere
   ```
   Resolve conflicts keeping the fork's behaviour, and fit the change to the fork's structure (an upstream fix to a
   UI file usually has to be redone in our file by hand). A commit that won't cherry-pick cleanly is often easier to
   port by reading `git show <hash>` and writing the change. Bring its tests along, adapted. A crate upstream
   **added** is not known to `rename-crates`: rename its package to `lcc-<name>`, register it in
   `xtask/src/layers.rs`, and run the command again.
4. **What's New:** an upstream feature worth telling users about gets one of our entries (`docs/whats-new/` and,
   if needed, `docs/changelog/unreleased/`), in our words. `docs/whats-new.md` (upstream's history up to the last
   merge) stays as it is: `cargo xtask notes --check` fails if it changes.
5. **Check and land** as any change: `cargo xtask ci --fast` plus the targeted tests. In a PR-intake round the
   commit goes into that round's single PR (see the runbook), and the PR merges at a version release; a commit
   reviewed only in the commit lane joins the next round. Mention the upstream commit(s) in the PR (the
   `cherry picked from` lines do this per commit).
6. **Record the review** once every listed commit is ported or decided against:
   ```sh
   cargo xtask upstream-review --done <hash>   # the last upstream commit reviewed (the command prints it)
   ```
   Commit the updated `upstream-reviewed.txt` (with the ports, or on its own).

## Sending fixes upstream

Upstream may still want our generic fixes. That is optional now and doesn't depend on merging:

- **`cargo xtask upstream-pr <branch> <commit>...`** creates `<branch>` off `upstream/main` and replays the commits
  (`git am --3way`) with our crate names mapped back to upstream's (`lcc-x` → upstream's prefix, `-p lcc-app` →
  upstream's app package). It never pushes: it prints the push/PR steps. Our copy of the commit then carries
  `UPSTREAM-PR: <link>` in its message. A What's New entry in those commits is a file in `docs/whats-new/`, which
  upstream doesn't read: on the upstream branch, move its lines into upstream's `docs/whats-new.md` by hand.
- **`cargo xtask shared-check [--strict]`** lists commits since the last merge that change shared paths without an
  `UPSTREAM-PR:` line. It is no longer a `ci` step (most fork changes now stay in the fork); run it when choosing
  what to offer upstream. [`upstream-candidates.md`](upstream-candidates.md) lists earlier candidates.

## Notes

- Upstream names are mapped by the brand tooling (`rename-crates --upstream`, `brand check`), never by
  hand-editing names; if a pattern keeps coming back, teach the tooling instead.
- Licence: upstream code arrives under MIT OR Apache-2.0; we use it under MIT and keep upstream's copyright lines in
  `LICENSE`/`NOTICE`. `crates/segment` and `crates/fetch` stay Apache-2.0. `cargo deny check licenses` (part of
  `cargo xtask ci`) catches a dependency a port brings in under a licence `deny.toml` does not allow.
- To change our own crate prefix (an internal codename users never see), run `cargo xtask rename-crates <prefix>` in
  a commit of its own, then re-check that `--upstream` still maps cleanly.
- **The old merge routine** (`cargo xtask upstream-merge [--ref REF] [--no-ci]`: a `merge/upstream-YYYYMMDD` branch,
  owned paths kept as ours, a list of what was set aside, crate names mapped, `ci`) still works, for an exceptional
  catch-up only. Expect most conflicts in `crates/ui-egui/` and upstream's UI tests; see PR #486 for how that went.
