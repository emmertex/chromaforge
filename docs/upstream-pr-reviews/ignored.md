# Upstream PRs ignored

Reviewed 2026-10-30: off brand, tied to upstream-only code (e.g. the upstream updater crate), or likely to regress the fork. Not ported; reopen only if upstream changes the PR materially.

| PR | Title | Reason |
|---|---|---|
| 3307 | Contributors: add my entry | Upstream About-window contributors list only |
| 3314 | Issue #3254: README note for Windows update failures | Upstream installer text; fork packaging differs |
| 3358 | deps: bump tar, zip, roxmltree, rustls-webpki and symphonia | Lockfile-only; also downgrades windows-sys and getrandom; fork bumps its own deps |
| 3362 | Update prompt shows the app icon instead of a letter | Depends on upstream's update crate and product name |
| 3365 | Fix pretend-version testing for update prompts | Same upstream update crate |
| 3339 | LR-PROF-CAMERA: fitted looks for the Fujifilm X-T5 | Stacked on unmerged #2445; third-party camera-measurement assets with unconfirmed provenance (clean-room risk) |
| 3356 | CI: workflows only call craft-common-actions at @main | Upstream CI/agent-file change; would overwrite the fork's CLAUDE.md |
