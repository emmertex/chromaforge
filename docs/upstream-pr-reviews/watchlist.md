# Upstream PRs on watch

Open upstream PRs we are not taking yet, but will take as soon as upstream signals they are good.

**Rule:** when upstream approves a watched PR for merge (an approving review) or merges it, treat it as vetted by
upstream and port it without a fresh full review. We still run the clean-room, brand, never-crash and asset checks
that every port gets, and test on GitHub only. A PR that stays open and unapproved stays here; one upstream closes
without merging leaves the list (record it in `closed-unmerged.md`). Move a PR's row to the matching review file once
it is ported.

Checked every 15 minutes while a release or merge round is running, otherwise at the weekly review
(`docs/upstream-merge.md`).

| PR | Title | Added | Upstream state when added |
|---|---|---|---|
| 3232 | Fix/nr shadow luminance bounds | 2026-10-30 | open, draft |
| 3298 | Issue #3272: AI masks are enlarged along the photo's edges, no outline under a raised Exposure | 2026-10-30 | open (reviewed r6: SKIP for now) |
| 3266 | LRC-DEV-TONAL: consolidate tonal processing as V3 | 2026-10-30 | open |
| 3279 | LR-EDIT-DETAIL-DENOISE: AI Denoise for finished pictures (JPEG, PNG, TIFF …) with a model of their own | 2026-10-30 | open |
| 3194 | LR-MASK-BRUSH: preserve Lightroom brush feathering and erasures | 2026-10-30 | open |
| 3231 | Issue #1337: restart instead of stopping when the window's GPU device is lost | 2026-10-30 | open, draft |
| 3307 | Contributors: add my entry (Peaperfish) | 2026-10-30 | open |
| 3308 | LR-BEHAV-TEXTFIELD: a text box's edit is saved to what it was typed for, and a save run at close ends it | 2026-10-30 | open |
| 3309 | Reject malformed base64 in MCP and segmask decoders | 2026-10-30 | open |
| 3312 | MCP: render_photo returns the image inline when connected to a remote app | 2026-10-30 | open |
| 3314 | Issue #3254: README explains how to recover a Windows update that fails with 1612/1714/1603 | 2026-10-30 | open |
| 3315 | Issue #3249: a face set aside as "not this person" stays set aside | 2026-10-30 | open |
| 3316 | Issue #2019: Open Photo from a right click on a face tile | 2026-10-30 | open |
| 3317 | FACES: a person's page is kept until a face changes, jumps to its suggestions, and People keeps its place | 2026-10-30 | open |
| 3318 | FACES: a person's page offers only faces clearly more like them, and can ignore their odd faces | 2026-10-30 | open |
| 3319 | Issue #3075: say where a raw's black and white levels came from | 2026-10-30 | open |
| 3321 | LR-IMP-FORMATS: decode compressed RAF 13–17 % faster with a buffered bit reader | 2026-10-30 | open |
| 3322 | LRC-LIB-CATALOG-SWITCH: a library missing at launch is reported instead of being made anew with demo photos (#3239) | 2026-10-30 | open |
| 3323 | LR-MASK-PINS: Object and Describe masks show a pin that selects them (#3049) | 2026-10-30 | open |
| 3324 | KEY-NEXTPREV: the mouse's Back and Forward buttons go to the previous and next photo (#3303) | 2026-10-30 | open |
| 3325 | LRC-LIB-FOLDERS: Move Folder To… asks for the destination without freezing the window (#3140) | 2026-10-30 | open |
| 3326 | Windows: a screenshot target swapped for a same-length file under NTFS tunneling is refused (add the file index to its identity) | 2026-10-30 | open |
| 3328 | Issue #3251: People cards show the full photo count, once | 2026-10-30 | open |
| 3329 | Issue #3252: a thumbnail cache of any size, Custom… in GB | 2026-10-30 | open |
| 3330 | LR-META-FILMSIM: read Fujifilm film simulation from the maker note | 2026-10-30 | open |
| 3331 | LR-VIEW-HISTOGRAM: the histogram's clipping buttons read in every theme | 2026-10-30 | open |
| 3332 | LR-BEHAV-TITLEBAR: a dialog leaves the macOS title bar undimmed | 2026-10-30 | open |
| 3335 | MENU-EDIT-SELECTBY: Select by lists each star rating on its own (#3311) | 2026-10-30 | open |
| 3339 | LR-PROF-CAMERA: fitted looks for the Fujifilm X-T5's film simulations (builds on #2445) | 2026-10-30 | open |
| 3341 | LR-BEHAV-BG-TASKS: starting a background export no longer freezes the window | 2026-10-30 | open |
| 3343 | LR-IMP-LOCAL: Choose… and Browse Folder… open their file dialog without freezing the window | 2026-10-30 | open |
| 3344 | LRC-DEV-VIEWOPTIONS: a render that stays slow says "Rendering…" even with the loading message off | 2026-10-30 | open |
| 3345 | LR-VIEW-PHOTOGRID: a thumbnail still loading shows a quiet cue, unlike an unreadable one | 2026-10-30 | open |
| 3347 | LR-META-EXIF-FULL: All Metadata shows the selected photos' shared fields and marks the others Mixed (gap 30) | 2026-10-30 | open |
| 3348 | LR-META-XMP: automatic XMP writes waiting for their folder show a row in the activity stack | 2026-10-30 | open |
| 3351 | LR-BEHAV-BG-TASKS: AI Denoise and Finding faces can be stopped, and Quit names every running job | 2026-10-30 | open |
| 3352 | Issue #3320: a render size far beyond the photo is held to a 512-megapixel ceiling instead of aborting | 2026-10-30 | open |
| 3356 | CI: workflows only call craft-common-actions at @main (AGENTS.md; windows-arm64 unpinned) | 2026-10-30 | open |
| 3358 | deps: bump tar, zip, roxmltree, rustls-webpki and symphonia to latest | 2026-10-30 | open |
| 3361 | LR-IMP-LOCAL: the print, display, slideshow template and soft proof file dialogs open without freezing the window (stacked on #3343) | 2026-10-30 | open |
| 3362 | Update prompt shows the app icon instead of a letter | 2026-10-30 | open |
| 3363 | LRC-LIB-LIBFOLDERS: Synchronize several folders at once, one after another (stacked on #3107 and #1943) | 2026-10-30 | open |
| 3364 | fix(ui): translate the white balance preset menu | 2026-10-30 | open |
| 3365 | Fix pretend-version testing for update prompts | 2026-10-30 | open |
| 3366 | Reject mismatched XML closing tags | 2026-10-30 | open |
| 3367 | Reject multiple GPX document roots | 2026-10-30 | open |
| 3369 | Issue #3368: drop_import's launch-list tests wait for the library checks before clicking | 2026-10-30 | open |
| 3370 | GPU equivalence tests run on a software adapter when asked (lavapipe, WARP), for local runs on every platform | 2026-10-30 | open |

Check with:

```sh
gh pr view <n> --repo <upstream repo> --json state,isDraft,reviewDecision,mergedAt
```
