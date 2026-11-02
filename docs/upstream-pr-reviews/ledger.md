# Upstream intake ledger

Single source of truth for what we decided about each upstream PR and commit. The procedure is
[`docs/upstream-pr-intake.md`](../upstream-pr-intake.md); the older files in this directory are the **frozen archive**
of the 2026-10 reviews and are not edited.

## Boundaries

| Lane | Reviewed up to | Date |
|---|---|---|
| Open and merged PRs | #3306 (`open-2930-up.md`) | 2026-10-29 |
| Closed, never merged PRs | #2852 (`closed-unmerged.md`) | 2026-10-26 |
| Commits on `main` | `77c0a6a` (`upstream-main-commits.md`) | 2026-10-29 |
| Upstream `main` at the last seeding | `5a09d07530` | 2026-11-01 |

The next round starts with every PR updated since 2026-10-29 plus every open PR above #3306, every closed PR updated
since, and every `PENDING`/`WATCH` row below. Use `python3 tools/upstream_intake.py enumerate --since 2026-10-29`.

## Ledger

`State` is as observed at review: `open`, `closed`, `merged` or `commit`. `Decision`: `ADOPT`, `LEARN`, `SKIP`,
`PENDING`, `WATCH`. `Recheck` says what re-opens the row (`-` for a final decision). `Upstream ref` is the reviewed
PR head or commit SHA. A pipe inside a cell is escaped with a backslash.

| Item | State | Title | Decision | Recheck | Reason | Upstream ref | Reviewed | Fork |
|---|---|---|---|---|---|---|---|---|
| #3232 | open | Fix/nr shadow luminance bounds | WATCH | upstream approval | Open draft; take once upstream approves or merges (watchlist rule) | `0c36dc6ee3` | 2026-11-02 @ 5a09d07530 | - |
| #3266 | open | LRC-DEV-TONAL: consolidate tonal processing as V3 | WATCH | upstream approval | Open; take once upstream approves or merges (watchlist rule) | `093b2e7c47` | 2026-11-02 @ 5a09d07530 | - |
| #3279 | open | LR-EDIT-DETAIL-DENOISE: AI Denoise for finished pictures (JPEG, PNG, TIFF …) with a model of their own | WATCH | upstream approval | Open; take once upstream approves or merges (watchlist rule) | `a2043ed13d` | 2026-11-02 @ 5a09d07530 | - |
| #3194 | open | LR-MASK-BRUSH: preserve Lightroom brush feathering and erasures | WATCH | upstream approval | Open; take once upstream approves or merges (watchlist rule) | `55b4e49be5` | 2026-11-02 @ 5a09d07530 | - |
| #3231 | open | Issue #1337: restart instead of stopping when the window's GPU device is lost | WATCH | upstream approval | Open draft; take once upstream approves or merges (watchlist rule) | `d1911d5d96` | 2026-11-02 @ 5a09d07530 | - |
| #3298 | merged | Issue #3272: AI masks are enlarged along the photo's edges, no outline under a raised Exposure | PENDING | next round: read the merged commits on `main` | r6 said SKIP for now (mask refinement needs a measured run); merged 2026-10-31 in integration r3, so decide it as commits | `f14d166fdf` | 2026-11-02 @ 5a09d07530 | - |

## Run log

| Run | Date | Upstream `main` | Candidates | Adopted | Pending/Watch | PR |
|---|---|---|---|---|---|---|
| 0 | 2026-11-02 | `5a09d07530` | seeded from `watchlist.md` | 0 | 6 | - |
