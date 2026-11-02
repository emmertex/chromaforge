# Contributing to Chromaforge

Thank you for helping. Humans and agents follow the same rules: [`AGENTS.md`](AGENTS.md) has them (clean-room,
no Adobe assets, pure Rust, never crash, everything is a command, the name lives in `brand.toml`). This page is the
workflow around them: how to pick something, claim it and get it merged. Questions, ideas, or a bug you'd like to
talk through first: [join the Discord](https://discord.gg/c3dfYATzsQ) and bring them to
[the project's channel](https://discord.com/channels/1359579021108842617/1559361045468942346).

## Pick something

- **Gaps:** [`docs/gaps.md`](docs/gaps.md) lists every known shortfall against Lightroom, ranked by user impact
  (`B` marks a beta blocker). [`docs/parity-checklist.md`](docs/parity-checklist.md) has one row per feature, menu
  item and shortcut.
- **Issues:** this repository's GitHub issues, bugs and feature requests alike.
- **Check nobody has it yet.** Read the issue's comments for a claim and search the open pull requests for its number
  and its checklist id. Claims live in comments.
- **Check upstream's work too.** Chromaforge no longer merges upstream: each round reviews upstream's pull requests
  and commits and ports the useful ones by hand ([`docs/upstream-pr-intake.md`](docs/upstream-pr-intake.md)), and
  the whole round lands as one fork PR at a version release. A bug fixed upstream may already be ported, or due to
  be: search the log and the open pull requests for the upstream issue or pull request number before you start.

## Claim it

Comment on the issue to say you're taking it, then go ahead. If there is no issue yet, open one (what's wrong or
missing, and how you know), so there is something to claim and for the pull request to close; a checklist row with
no issue is named in the pull request's title instead. If you stop, say so in the issue so someone else can pick it
up.

## Open a pull request

- **From a fork** (contributors don't push to this repository), branched from the latest `main`. GitHub merges a
  pull request only when its branch is up to date with `main`: when `main` moves while yours waits, merge it in
  (`git fetch origin && git merge origin/main`), run the checks again and push.
- **Fill in the template** ([`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md)). One change
  per pull request: unrelated fixes and clean-ups go in their own.
- **Title:** start with the task or checklist id, as the commits do: `LR-IMP-FORMATS: folder import finds Sony SR2
  and SRF files`, or `Issue #123: …` when there is no checklist row.
- **Description:** put `Fixes #123` in it when there is an issue, so merging closes it. A pull request that does
  only part of an issue says `Part of #123` instead. Say what you changed and how you checked it (tests, the app,
  screenshots for UI changes). If it builds on another open pull request, say so at the top.
- **Green first** ([`docs/ci.md`](docs/ci.md)): every commit passes `cargo xtask ci --fast` (fmt, clippy with the
  never-crash lints, and the other checks `docs/ci.md` lists), the SIL suite (`cargo xtask sil`) and
  `cargo test -p <crate>` for the crates you touched. New features come with a test that fails without them;
  crash fixes with a hostile-input check that the bad input gives an error, not a panic.
- **One task id per commit**, and the docs it moves in the same commit: the feature's rows in
  `docs/parity-checklist.md`, its entry in `docs/gaps.md`, and the numbers in `ROADMAP.md` /
  `docs/target-app-parity.md` when a listed gap closes.
- **Release notes:** a change people will notice adds its own file under `docs/whats-new/` (and, for the
  changelog, `docs/changelog/unreleased/`); the READMEs there say how.
- **Sources:** a pull request that adds an algorithm names what it was built from (a paper, a prose spec, or "own
  design") on a `Sources:` line. The clean-room rules in [`AGENTS.md`](AGENTS.md) say what may be read.
- **Assets:** every image, icon or font has an entry in [`assets/ATTRIBUTION.md`](assets/ATTRIBUTION.md), added in
  the same commit. Fonts go to [storytold/craft-fonts](https://github.com/storytold/craft-fonts), not here.
- **Licence:** what you contribute is offered under the licence of the code you change: MIT ([`LICENSE`](LICENSE)),
  except `crates/segment` and `crates/fetch`, which are Apache-2.0.
- CI runs on the maintainer's machines, so a pull request from a fork waits for its run to be approved: a red result
  can come back hours after you push.

## Credits

The About window credits everyone by GitHub username. If you'd like a real or display name shown as well, add your
own entry to `contributors/people.toml` in your pull request ([`docs/contributors.md`](docs/contributors.md)). Only
ever add or change your own entry.

## Working with an agent

An agent working for you follows [`AGENTS.md`](AGENTS.md) and this page, and you are the author of what it submits:
read its pull request before you open it, and answer review comments yourself or through it.
