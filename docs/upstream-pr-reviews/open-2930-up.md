# Upstream open PRs 2930 and up: review

Reviewed 2026-10-26 (rows from 3192 on: r6, 2026-10-29) against fork `main`. ADOPT = ported (adapted to the fork's names), LEARN = idea taken in
the fork's own form (or noted for later), SKIP = not taken. "Stacked" = built on another open upstream PR.

| PR | Title | Decision | Reason | Fork PR |
|---|---|---|---|---|
| 2952 | LR-EDIT-COLOR-TEMP: a one-field white balance partial keeps the other field's look (#2910) | ADOPT | a one-field white balance partial keeps the other field's look | fork #554 |
| 2963 | Accept a UTF-8 BOM at the start of the CLI --script files | ADOPT | scripts saved by Windows editors start with a BOM | fork #554 |
| 2964 | LR-IMP-ADD-DIALOG: handle rename sequence exhaustion | ADOPT | rename sequence exhaustion is an error, not a wrap | fork #554 |
| 2969 | Report a photo's custom white balance left out of an imported look | ADOPT | the import reports a custom white balance it left out | fork #554 |
| 2977 | LR-META-EXIF-FULL: reject malformed EXIF clocks | ADOPT | hostile EXIF clocks rejected (never-crash) | fork #554 |
| 2986 | Issue #2980: Clear Color Label can be given a keyboard shortcut | ADOPT | missing shortcut slot | fork #554 |
| 2942 | Issue #2936: build the GPU kernels on parallel threads | ADOPT | faster GPU start-up | fork #557 |
| 2989 | GPU-TESTS: every WGSL kernel parses and validates in the test run, without a GPU | ADOPT | WGSL kernels validated without a GPU | fork #557 |
| 2974 | LR-BEHAV-WINDOW: on Windows a maximized window opens maximized | ADOPT | Windows window state fix | fork #558 |
| 2979 | LRC-LIB-CATALOG-SWITCH: a library's catalog.snap or folder given at launch opens that library | ADOPT | a library given at launch opens | fork #558 |
| 2988 | Export: Lanczos-3 downscale and a Gaussian luma unsharp mask for crisper exports | ADOPT | crisper exports (Lanczos-3 downscale, luma unsharp mask) | fork #559 |
| 2966 | LR-EDIT-GEOM-UPRIGHT: Upright corrects posts among people, and says why it found nothing | ADOPT | measured on real frames: fewer frames made worse; Upright says why it found nothing | fork #561 |
| 2991 | KEYC-METADATA: Cmd+Alt+Shift+K / Ctrl+Alt+Shift+K sets the keyword shortcut, and Shift+K adds or removes it | LEARN | the fork already had a single keyword shortcut (⌥⇧K / ⇧K) in its library prefs; extended to several keywords, the dialog and the Keyword List mark instead of upstream's catalog-mark storage | fork #564 |
| 2955 | LR-EDIT-COLOR-MIXER-HSL: the Color Mixer's HSL mode, laid out as in Lightroom Classic | LEARN | the fork already had Mixer / Point Color tabs; added the HSL mode, groups and `view.colorMixer` onto them | fork #572 |
| 2932 | LR-EDIT-DETAIL-DENOISE: AI Denoise follows Auto Sync in one undo step, starts at import from an ISO, one batch row (stacked on #2929) | LEARN | builds on #2929 (fork #507, merged); AI Denoise follows Auto Sync in one undo step | fork #602 |
| 2982 | LR-EDIT-DETAIL-DENOISE: built-in BM3D denoising with GPU acceleration | LEARN | built-in BM3D denoising with GPU acceleration | fork #599 |
| 2971 | LR-IMP-DUPES: a photo browsed in Local is offered by Import and no longer blocks the library's own file | LEARN | a photo browsed in Local no longer counts as library content in the Import review | fork #579 |
| 2944 | Import review: say what the folder scan left out | LEARN | the import scan says what it left out | fork #589 |
| 2940 | Import review: an optional batch-edit recipe for newly imported photos | LEARN | optional batch-edit recipe in the import review | fork #594 |
| 2941 | LR-META-XMP: Save and Read Metadata can run on a whole folder, with a row in the activity stack | LEARN | Save / Read Metadata for a whole folder with an activity row | fork #583 |
| 2935 | LRC-LIB-LIBFOLDERS: a folder's menu has one grouped order in Folders and Local | LEARN | one grouped order for the folder menus | fork #592 |
| 2956 | LRC-LIB-LIBFOLDERS: a folder can be exported from its menu (stacked on #2935) | LEARN | Export Folder from its menu (stacked on #2935) | pending (stacked on fork #601) |
| 2959 | LRC-LIB-LIBFOLDERS: every folder menu can copy the folder's path and make a folder on disk (stacked on #2935) | LEARN | Copy Path and New Folder on Disk in every folder menu (stacked on #2935) | fork #601 |
| 2961 | LRC-LIB-LIBFOLDERS: a folder's menu can bring photos in (stacked on #2935) | LEARN | Import to This Folder from a folder's menu (stacked on #2935) | ported (fork #820) |
| 2981 | Issue #2819: the histogram stays SDR when HDR is switched off | SKIP | already fixed in the fork | — |
| 2973 | MENU-BAR: clippy passes on macOS again (Quit, About and Settings share one branch) | SKIP | the fork's native_menu differs; its macOS clippy already passes | — |
| 2954 | LR-IMP-ADD-DIALOG: an import checks its probes and looks up sidecars several at a time | SKIP | the fork already looks up probes and sidecars in parallel | — |
| 2972 | chore(deps): bump the cargo-dependencies group across 1 directory with 16 updates | SKIP | dependency bump; the fork bumps its own dependencies | — |
| 2994 | chore(deps): refresh safe dependency bumps | SKIP | dependency bump; the fork bumps its own dependencies | — |
| 2264 | ci(deps): bump cachix/install-nix-action from 31.11.1 to 31.11.2 in the actions group across 1 directory | SKIP | dependency bump (closed upstream) | — |
| 2265 | chore(deps): bump the cargo-dependencies group across 1 directory with 17 updates | SKIP | dependency bump (closed upstream) | — |
| 2642 | chore(deps): bump the cargo-dependencies group across 1 directory with 18 updates | SKIP | dependency bump (closed upstream) | — |
| 2948 | PERF-CATALOG: a library states what its missing develop settings read as (stacked on #2867) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2951 | PERF-CATALOG: the catalog formats are one table, and every format from 6 on has a fixture (stacked on #2948) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2957 | PERF-CATALOG: develop settings can be stored as their difference from the defaults (stacked on #2951) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2958 | PERF-CATALOG: a put-off upgrade keeps one copy per attempt that worked and none for one that did not (stacked on #2968) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2968 | PERF-CATALOG: catalog records state only what was set (catalog format 10) (stacked on #2957) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2978 | PERF-CATALOG: the segments of a snapshot, with no caller yet (stacked on #2958 and #2871) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2983 | PERF-CATALOG: the catalog saves its photos in segment files and opens them on several threads (catalog format 11, stacked on #2978) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2984 | PERF-CATALOG: segment files nothing lists are removed when a library is opened, and segments without a snapshot are refused (stacked on #2983) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2985 | PERF-CATALOG: a compaction writes only the segments whose photos changed (stacked on #2984) | SKIP | PERF-CATALOG stack: the fork has its own v4 store | — |
| 2931 | LR-LIB-DELETE: photo.trashFiles moves photos' files to the system Trash (stacked on #2927) | SKIP | stacked on #2927, which the fork skipped | — |
| 2934 | LR-LIB-DELETE: Move Files to Trash can run in the background, and the commands that move files wait while it does (stacked on #2931) | SKIP | stacked on #2931 (base #2927 skipped) | — |
| 2943 | LR-LIB-DELETE: Move Files to Trash… asks, moves the files in the background, and asks again before erasing (stacked on #2934) | SKIP | stacked on #2934 (base #2927 skipped) | — |
| 2930 | Top bar: the title-bar drag uses craft-window's title_bar; a panel over the bar keeps the window still | SKIP | needs egui 0.37, deferred (plan in upstream-main-commits.md) | — |
| 2950 | Native iPad app (ios-port) | SKIP | out of scope (native iPad app) | — |
| 2947 | HW-SURFACE: control surfaces (Logitech MX Creative Console), engine ported from TactileBridge | SKIP | out of scope (hardware control surfaces) | — |
| 2975 | Preserve queued file drops across dialogs and restore Flatpak sources | SKIP | out of scope (Flatpak / drop-queue rework) | — |
| 2946 | LR-LIB-FOLDER: cargo xtask refusals keeps new silent command calls out of the interface (stacked on #777) | SKIP | stacked on a draft base (#777) | — |
| 2945 | Issue #1184: the template allows a tested core whose caller comes in the next pull request | SKIP | upstream PR template only | — |
| 2987 | build: pin craft-update to craft-libs main (495e597) | SKIP | craft-update pin for upstream's build (closed) | — |

## Closed

Closed upstream without merging.

| PR | Title | Decision | Reason | Fork PR |
| 2949 | LR-PREF-GENERAL: the Settings tab bar, filter, choices and questions can be read in every theme (stacked on #2906) | SKIP | draft stacked on a chain of unmerged settings/theme PRs; revisit once they land |  |
| 2998 | KEYC-COMPARE: Shift+Up/Down and Ctrl+Shift+arrows extend the grid selection | ADOPT | keyboard selection of grid rows | fork #611 |
| 3002 | LR-META-EXIF: TIFF keeps shared child IFDs and multi-string ASCII values | ADOPT | mostly already in Chromaforge; the copy's entry and byte charge added | fork #609 |
| 3003 | LR-EDIT-EFFECTS-GRAIN: grain skips a degenerate frame instead of producing NaN pixels | SKIP | already in Chromaforge |  |
| 3004 | LR-IMP-FORMATS: decode compressed Samsung SRW (32770, 32772, 32773) | SKIP | Chromaforge already decodes compressed SRW (`vendor::srwc`) |  |
| 3005 | fix(gpu): LR-BEHAV-GPU: bounded blur work per pixel so large radii can't trigger a GPU reset | SKIP | already in Chromaforge |  |
| 3006 | CLI-HOSTILE: render --set refuses inf / NaN instead of dropping the value | SKIP | already in Chromaforge |  |
| 3007 | Never crash: CR2 slice widths that overflow are an error; XMP attribute unescape in one pass | ADOPT | remaining parts: usize conversion of slice widths, leading-zero references | fork #609 |
| 3008 | security(raster): fallible image allocation; no panic on empty images or zip_map size mismatch | ADOPT | remaining parts: checked zip_map index, 32-bit-safe MAX_IMAGE_BYTES | fork #609 |
| 3009 | LR-MERGE-HDRPANO: a repeated identical HDR merge reuses the verified output | ADOPT | same idea as #435, already in Chromaforge; the `reused` result field added | fork #611 |
| 3010 | #366: SAM 3 mirrors accept a local folder; tokenizer files pinned | ADOPT | local mirrors and pins were in; `file://` percent escapes added | fork #611 |
| 3011 | security(codecs): refuse decodes with no pixels; checked JPEG sample counts | ADOPT | code already in; the zero-size header test added | fork #609 |
| 3012 | #2361: lens corrections crop inside the corrected photo | ADOPT | already in except the estimated-CA constrain | fork #611 |
| 3013 | LR-EDIT-OPTICS-PROFILE: lens profiles for compact cameras with a built-in lens | ADOPT | remaining parts: no-make cameras, overflow guard, exact-name lookup without the mount fallback | fork #611 |
| 3014 | MENU-CTX-GRID: the photo menu's rows, built from the menu bar's model | LEARN | a photo context menu built from the menu bar's model is the right shape; too large to port while the docking rework lands, revisit with #3026 |  |
| 3015 | Never crash: an oversize export size is an error, not a multiply overflow | SKIP | already in Chromaforge |  |
| 3016 | LRC-LIB-LIBFOLDERS: a date folder inside the library can be renamed | ADOPT | imported date folders can be renamed; refused prompts say why | fork #611 |
| 3017 | LR-IMP-FORMATS: decode AVIF on import (pure-Rust AV1 decoder) | SKIP | Chromaforge already decodes AVIF (`codecs::avif`) |  |
| 3018 | perf(catalog): box Photo::embedded_lens (~240 MB less per 1M photos) | LEARN | worthwhile memory saving; touches many call sites, do it with the next catalog memory pass |  |
| 3019 | LR-BEHAV-WINDOW: remember where the window was, not only its size (#2967) | SKIP | already in Chromaforge (`window_place`) |  |
| 3020 | Originals: on Windows a hard link to an original counts as that original | ADOPT | Windows hard links via the same-file crate | fork #611 |
| 3021 | Clean-room Canon CRW decode, EOS D60 colour matrices | SKIP | Chromaforge already decodes CRW (`vendor::crw`) and ships dcraw's matrices |  |
| 3023 | LR-MASK-OBJECTS: recover from SAM worker accept errors | SKIP | already in Chromaforge (accept errors are logged and retried) |  |
| 3024 | LRC-LIB-PEOPLE: Start the face scan's progress over in another library (#2995) | ADOPT | progress row resets with the library | fork #611 |
| 3026 | MENU-CTX-GRID: the photo menu offers the Photo menu's actions, grouped by task (stacked on #3014) | LEARN | see #3014 |  |
| 3027 | LR-PROF-CAMERACOLOR: docs count 9 bundled camera profiles, not 5 | SKIP | upstream's doc counts; Chromaforge's docs count its own bundle |  |
| 3030 | LR-BEHAV-MULTISELECT: click selection as in Lightroom Classic (frame click selects alone, Shift/Ctrl keep the active photo) | LEARN | Classic click semantics worth matching; grid/filmstrip code is being reworked by the docking cherry-picks, port after |  |
| 3031 | KEY-INFO, LR-BEHAV-PANEL-RESIZE: panels come back as they were (closing a tool, pressing a panel key again) | LEARN | panel restore behaviour; collides with the docking rework, revisit after it lands |  |
| 3034 | LR-PREF-INTERFACE: Settings › Interface size, Auto or 50–300% | LEARN | an interface size setting is useful; settings UI is in flux with the docking/craft-ui work, revisit |  |
| 3035 | i18n(it): launch library chooser, Lightroom import destination and new settings | ADOPT | only the strings Chromaforge has, product name as `{app}` | fork #611 |
|---|---|---|---|---|
| 435 | Reuse verified HDR merge results without changing existing edits | LEARN | reuse an identical HDR merge result instead of writing a duplicate DNG (#430); upstream asked for a minimal fix and closed it: worth doing in the fork as that minimal fix; already in Chromaforge (commits 101efb1ae, 6459531cf), `reused` field added with #3009 | fork #611 |
| 2999 | Clean room crw format decode | SKIP | closed; Chromaforge already decodes CRW | |
| 1589 | Shrink the app chrome to PhotoCraft control scale | SKIP | shrinks the chrome to another app's control scale; closed upstream pending its scale decision (#2962), and the fork keeps the Lightroom-parity scale | — |
| 1848 | gpu: renders queue for device memory; out of memory never retires the GPU (#1764) | LEARN | GPU out-of-memory should queue renders and fall back to the CPU instead of retiring the GPU (#1764); closed unmerged; the fork has its own `crates/gpumem` budget, revisit if OOM retirements are reported; already in Chromaforge as `crates/gpu/src/budget.rs` (commit 6c4864f6a) | in main (6c4864f6a) |

## Review r4 (2026-10-27): open PRs not yet listed, up to 3119

| PR | Title | Decision | Reason | Fork PR |
|---|---|---|---|---|
| 3038 | macOS: CFBundleLocalizations | ADOPT | AppKit's own dialogs follow the app's languages | this bundle |
| 3041 | Detail startup view for launch files | LEARN | the fork's startup path differs; port only if it shows the same bug | — |
| 3042 | Windows: gvfs unused variable | SKIP | already in Chromaforge | — |
| 3043 | Dependabot bumps (quick-xml, jixel, candle 0.11) | SKIP | Chromaforge pins its own versions; candle / jixel bumps are a regression risk | — |
| 3045 | Windows: folder trees hide hidden folders | ADOPT | small isolated fix; non-Windows unchanged | this bundle |
| 3047 | Web: craftStorage | SKIP | needs craft-libs' `craft-web-host` | — |
| 3054 | Reject invalid XMP GPS subdivisions | ADOPT | `crates/meta` validation only; its UI / locale parts left out | this bundle |
| 3055 | PERF-CULL: wedding culling | LEARN | large multi-crate perf work; mine piecewise later | — |
| 3073 | Lens DB auto-download on first run | LEARN | Chromaforge has its own `lens_db` commands; upstream's adds network on first run and a catalog format bump | — |
| 3074 | Integration: 23 approved PRs | SKIP | a bundle of other PRs, reviewed one by one | — |
| 3076 | `--connect` resends a command that crossed the idle-close notice (#3069) | ADOPT | control client / MCP retry bug fix | this bundle |
| 3077 | Culling: loupe uses built previews | SKIP | stacked on #3055 | — |
| 3078 | Text box: an outside press ends the edit with the menu open | LEARN | the fork's `TextField` diverged; recheck there | — |
| 3079 | `cargo xtask test` under ci's caps (#3058) | SKIP | upstream test infrastructure | — |
| 3080 | Culling: camera JPEG first | SKIP | stacked on #3077 | — |
| 3083 | testkit deadline helpers and watchdog (#3062) | LEARN | bounded test waits; upstream's harness | — |
| 3084 | Settings: a tab click keeps typed text | SKIP | Settings redesigned in Chromaforge | — |
| 3085 | CI: simplify clippy-flagged conditions | SKIP | Chromaforge's clippy doesn't flag these lines | — |
| 3086 | Plain cargo test on 4 threads (#3059) | SKIP | test infrastructure | — |
| 3087 | wgpu compatibility fork for OCLP Macs (#1939) | SKIP | forked wgpu dependency | — |
| 3088 | Settings test restores the memory budget (#3065) | SKIP | upstream test harness | — |
| 3089 | An album shown is not also a filter | LEARN | check the fork's filter model first | — |
| 3090 | Export-dialog test waits with the background bound | SKIP | upstream test flake | — |
| 3091 | SAM 3 model can be removed in Settings | LEARN | engine remove command worth having; Settings UI is Chromaforge's | — |
| 3092 | YuNet looks at photos up to 1024 px in their own shape | ADOPT | finds small faces; isolated to `crates/faces` | this bundle |
| 3093 | Undo of a keyword delete / folder removal restores filters | LEARN | depends on upstream's #2758 filter work | — |
| 3095 | Settings tab opened by a command keeps text | SKIP | Settings redesigned in Chromaforge | — |
| 3096 | Tests stop asserting wall-clock time (#3064) | SKIP | test infrastructure | — |
| 3097 | CR3 white level saturation (#2053) | ADOPT | CR3 highlights clipped at the real sensor ceiling | this bundle |
| 3098 | CLI refused-connection tests use a free port (#3070) | ADOPT | 18437 clashes with agents' control ports | this bundle |
| 3099 | xtask bounds each test binary (#3060) | SKIP | draft test infrastructure | — |
| 3100 | ui-egui test caches sized (#3066) | SKIP | draft; its own PR doubts the value | — |
| 3102 | AGENTS.md test rules (#3061) | SKIP | upstream docs | — |
| 3104 | Windows installation policy | SKIP | upstream packaging (draft) | — |
| 3105 | JPEG size limit reports its quality (#3033) | LEARN | engine report worth having; the export dialog is Chromaforge's | — |
| 3106 | Sidebar multi-select | SKIP | stacked on #1153 / #2707 | — |
| 3107 | Sidebar multi-select actions | SKIP | stacked on #3106 | — |
| 3108 | Cut / Copy / Paste albums and keywords | SKIP | stacked on #3107 | — |
| 3109 | SAM 3 masking, model preload, a6400 lens | SKIP | mixed PR; conflicts with Chromaforge's segment work | — |
| 3110 | ai-bench: WIDER FACE | SKIP | benchmark tooling | — |
| 3111 | Enforce coding standards via clippy | SKIP | workspace lint policy change | — |
| 3112 | ai-bench: LFW | SKIP | benchmark tooling | — |
| 3113 | ui-egui test apps render on two workers (#3067) | LEARN | test memory; check the production render pool is unchanged | — |
| 3115 | Batch keyword delete / move by level | SKIP | stacked on #2757 | — |
| 3116 | Transparent sources keep alpha in PNG, TIFF and WebP (#2976) | ADOPT | export correctness | this bundle |
| 3117 | GPU device creation gives up after a time limit (#3068) | ADOPT | hang fix | this bundle |
| 3118 | Memory sizing honours cgroup limits (#3072) | ADOPT | containers, Flatpak, systemd scopes | this bundle |
| 3119 | A cut-short folder move resumes at the next open | SKIP | stacked on #2914 / #1925 | — |
| 3120 | Issue #3063: a test settle that times out fails with what was still busy; settle is must_use | SKIP | upstream test harness | — |
| 3121 | PERF-CATALOG: a smart album's per-photo test stops storing an answer nothing can read | LEARN | smart album perf | — |
| 3122 | LC-002: reject non-object script parameters | ADOPT | non-object script params refused | branch `upr/hardening-3122-3165` |
| 3123 | LC-001: stop commands after failed imports | LEARN | stop a script after a failed import: worth a fork take | — |
| 3124 | LC-003: reject non-UTF-8 CLI arguments without panicking | ADOPT | non-UTF-8 CLI argument refused, not a panic (desktop app too) | branch `upr/hardening-3122-3165` |
| 3125 | MENU-BAR: fix macOS clippy failure | SKIP | closed upstream | — |
| 3126 | Issue #3071: rayon's global pool and the ad-hoc worker threads have names | SKIP | thread names: minor | — |
| 3134 | fix(meta): read the DateTime offset with unsigned_abs (#3132) | SKIP | already in the fork (`unsigned_abs`) | — |
| 3135 | fix(meta): cap Rational and SRational tag rows at 16 values like every array (#3133) | SKIP | the fork has no `file_tag_rows` value formatter | — |
| 3136 | LR-CI: fix Rust 1.95 lints and standard-font menu hints | SKIP | upstream lints / UI hints | — |
| 3137 | LR-IMP-LOCAL: update capture time on reload and auto-reload changed files in local folders | LEARN | reload updates capture time | — |
| 3138 | LR-PROF-CAMERACOLOR: align CR3 calibration through resolved lens profiles | LEARN | CR3 calibration via lens profiles | — |
| 3139 | Add GFX100S II to the spectral camera matrix table | LEARN | GFX100S II spectral matrix: check against camdb sources | — |
| 3141 | Issue #2589: a Lightroom import never creates a photo already in Recently Deleted | LEARN | Lightroom import vs Recently Deleted | — |
| 3142 | Issue #2589: a Lightroom import says where each catalog photo went | LEARN | Lightroom import reports placements | — |
| 3143 | Issue #3114: Lightroom catalogs import into libraries on exFAT drives | LEARN | Lightroom import on exFAT | — |
| 3144 | LR-EDIT-OPTICS-PROFILE: enable Sony ILCE-7M3 embedded distortion correction (rebase of #1384) | LEARN | Sony ILCE-7M3 embedded distortion | — |
| 3145 | Issue #2165: a Lightroom collection that can't nest goes under the nearest set instead of failing the import | LEARN | Lightroom collection nesting fallback | — |
| 3146 | LR-BEHAV-LOCALE: the desktop app starts in the system's language | LEARN | start in system language | — |
| 3147 | i18n(ru): 217 strings the Russian catalog was missing | SKIP | translation for upstream's catalogs | — |
| 3148 | Issue #3056: Paste Selected Settings pastes crop and Remove left out of the copy | LEARN | Paste Selected Settings crop / Remove: engine fix worth porting; touches 30 files incl. UI | — |
| 3150 | LR-BEHAV-BG-TASKS: exports and imports say how they ended (stacked on #1711) | SKIP | stacked (#1711) | — |
| 3151 | LR-BEHAV-BG-TASKS: every other job says how it ended (stacked on #3150) | SKIP | stacked (#3150) | — |
| 3152 | Issue #1588: one bad Lightroom record costs that record, not the import | LEARN | one bad Lightroom record costs only that record | — |
| 3154 | fix(ui): dropdown menus grow back after a search with no matches | SKIP | upstream album_picker widget, not in the fork | — |
| 3155 | LR-UI-DENSE-ROWS: one-line slider rows with a fitted, right-aligned name column | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3156 | LR-BEHAV-TEXTFIELD: an edit in progress is saved however its field is left, except Esc | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3157 | LRC-LIB-FILTERBAR: every filter change goes through one step that leaves out what the source shown owns | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3158 | LR-UI-TOOL-ROW: tools in a row under the histogram, histogram fixed above the scrolling panel | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3162 | Integration: 7 approved PRs (catalog, import and folders) | SKIP | integration batch | — |
| 3163 | Integration: 6 approved PRs (drag and drop, filmstrip and panels) | SKIP | integration batch | — |
| 3164 | LR-UI-TRACK-SHADE: grey slider tracks show which way they take the photo | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3165 | Reject signed list indexes in indexed develop control ids | ADOPT | signed list index in a control id refused | branch `upr/hardening-3122-3165` |
| 3166 | LRC-LIB-LIBFOLDERS: a folder made with New Folder on Disk… appears in Folders at once | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3167 | LR-BEHAV-DIALOGS: a one-line prompt that's refused stays open and says why | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3168 | Speed up full-size exports by removing copies and serial passes | ADOPT | export perf (2 of 3 commits; see upstream-main-commits.md) | branch `upr/export-perf-3168` |
| 3169 | LRC-LIB-DUPLICATES: Find Duplicates (exact copies and same-picture-saved-again, no AI model) | LEARN | Find Duplicates: large feature | — |
| 3170 | Integration: 7 approved PRs (catalog, import and folders, round 2) | SKIP | integration batch of other PRs | — |
| 3171 | MENU-APP-UPDATES: --no-update-check skips the startup update check | LEARN | --no-update-check | — |
| 3176 | LR-IMP-ADD-DIALOG: an import's probe opens each file once | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3179 | LR-BEHAV-SIDEBAR-KEYS: the keys' model for the left sidebar: the mode, its cursor and its ring (no caller yet) | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3180 | LR-BEHAV-SIDEBAR-KEYS: the left sidebar from the keyboard (stacked on #3179) | SKIP | stacked on #3179 | — |
| 3181 | feat(upstream OCR crate): ONNX runner and pinned PP-OCRv6 models for reading text in photos (part 1a) | SKIP | OCR model chain (ONNX, stacked) | — |
| 3182 | Disarm color picking when adding mask components | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3183 | Add Lithuanian interface translation. | SKIP | translation for upstream's catalogs | — |
| 3185 | Hidden entries: folder walks read no extra metadata on macOS; auto-import skips hidden files (follow-up to #3045) | LEARN | hidden-entry follow-up to #3045 (macOS) | — |
| 3186 | feat(upstream OCR crate): read text with PP-OCRv6 and a HierText benchmark example (part 1b, stacked on #3181) | SKIP | stacked on #3181 | — |
| 3188 | Drop the sign from slider text that rounds to zero | SKIP | UI change in fork-owned ui-egui, or no user-visible gain | — |
| 3103 | refactor: shared module layout (craft-contrib#20) | SKIP | upstream repository layout refactor | (r6 PR) |
| 3192 | Bump winit for Wayland gestures and drag-and-drop support | SKIP | winit bump; egui 0.37 migration deferred | (r6 PR) |
| 3194 | LR-MASK-BRUSH: preserve Lightroom brush feathering and erasures | SKIP | LR brush CenterWeight semantics changed on an unconfirmed reading; open and unverified against real sidecars | (r6 PR) |
| 3195 | Integration: 4 PRs (task queue, menus, accessibility, autosave) | SKIP | integration of 4 PRs, UI/menus | (r6 PR) |
| 3196 | MENU-BAR: the macOS menu bar keeps every row when two rows run the same command | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3197 | Issue #3191: encode DNG WhiteLevel tag as LONG instead of RATIONAL | SKIP | duplicate of 3199; rejects fractional white levels with an error, which would break float DNG export | (r6 PR) |
| 3198 | CI: satisfy Rust 1.95 Clippy checks | SKIP | clippy-only | (r6 PR) |
| 3199 | Issue #3191: write the DNG WhiteLevel as LONG, never RATIONAL | ADOPT | float DNG WhiteLevel written as LONG with fractional white rescaled (the safer of 3197/3199) | (r6 PR) |
| 3200 | MENU-BAR: shortcut hints on macOS list modifiers in the menu bar's order (⇧⌘I, not ⌘⇧I) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3201 | LR-BEHAV-LOCALE: the Edit menu drops its 18-row Language submenu; Settings has the choice | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3202 | MENU-HELP-HELP: on macOS Check for Updates… is in the app menu, and Help opens with the app Help (⇧⌘?) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3203 | MENU-WINDOW-BRINGFRONT: the macOS Window menu lists the open windows | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3204 | LR-BEHAV-TITLEBAR: center the macOS traffic lights on the top bar | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3206 | Issue #3175: Read Metadata from File can skip photos whose sidecar changes nothing | SKIP | feature (Read Metadata skip); not a fix | (r6 PR) |
| 3207 | LR-META-XMP: photo.sidecarPaths reports where each photo's sidecar is | SKIP | feature (sidecarPaths command); no user-visible gain | (r6 PR) |
| 3208 | LR-BEHAV-BG-TASKS: on macOS a running job keeps the Mac awake and the app out of App Nap | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3209 | LR-BEHAV-ACCESS: the sidebar's disclosure triangles name their row | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3210 | LR-IMP-FORMATS: reject invalid Canon CR2 slice geometry | ADOPT | CR2 slice geometry validated (raw part only; clippy-only files skipped) | (r6 PR) |
| 3211 | LR-BEHAV-ACCESS: Increase contrast follows macOS until you choose (stacked on #2373) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3212 | LR-BEHAV-ACCESS: a date header in the photo grid is named with the text it shows | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3213 | MENU-FILE-CLOSE: document that closing the window quits the app | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3214 | LR-BEHAV-ACCESS: a slider's value field is named "<slider> value" and describes how to use it | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3215 | LR-BEHAV-ACCESS: Reduce transparency follows macOS until you choose (stacked on #3211) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3216 | LR-BEHAV-ACCESS: a filmstrip thumbnail is named like the grid's cell for the same photo | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3217 | MENU-HELP-FEEDBACK: the macOS Help menu has one feedback row, the app's | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3218 | MENU-VIEW-ENTERFULLSCREEN: the macOS View menu has one window full-screen row, Enter or Exit Full Screen on ⌃⌘F | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3219 | LR-BEHAV-BG-TASKS: on macOS the Dock icon shows how many jobs are running (stacked on #3208) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3220 | MENU-HELP-HELP: on macOS the Help menu is registered with AppKit after the menu bar is installed | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3221 | LR-VIEW-PHOTOGRID: the grid's empty view names Import Photos' shortcut as the keymap has it | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3222 | LR-BEHAV-LOCALIZE: add Korean interface and README | SKIP | Korean interface; fork i18n catalogs differ | (r6 PR) |
| 3223 | LR-BEHAV-BG-TASKS: on macOS a notification tells you when a job has finished (stacked on #3219) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3224 | Scan the whole repository again in SonarCloud | SKIP | upstream CI (SonarCloud) | (r6 PR) |
| 3225 | MENU-EDIT-SELECTNONE: Edit lists Select Only Active Photo and Paste Settings from Previous with their kin, and the macOS Edit menu keeps single separators | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3226 | LR-BEHAV-TOAST: keep notifications readable in light themes | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3227 | LR-BEHAV-PANEL-RESIZE: close or group panels from tool icons | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3230 | Issue #3228: the hard-link originals test holds under SeBackupPrivilege (Git Bash, the CI gate) | SKIP | Windows-only test tweak | (r6 PR) |
| 3231 | Issue #1337: restart instead of stopping when the window's GPU device is lost | SKIP | windowed GPU-device-loss restart; cannot be verified unattended | (r6 PR) |
| 3232 | Fix/nr shadow luminance bounds | SKIP | 485-line GPU/pipeline noise-reduction change; too large to take unrun | (r6 PR) |
| 3233 | Reject signed ports in Url::parse | ADOPT | signed ports refused (fetch) | (r6 PR) |
| 3234 | Reject signed numbers in Content-Range parsing | ADOPT | signed Content-Range numbers refused (fetch) | (r6 PR) |
| 3235 | Show the remembered crop ratio's box on an untouched photo | SKIP | UI change in fork-owned ui-egui | (r6 PR) |
| 3236 | chore(deps): bump the cargo-dependencies group across 1 directory with 7 updates | SKIP | dependency bump group; fork manages its own deps | (r6 PR) |
| 3237 | Load face embedding caches larger than 1 GiB instead of rescanning | ADOPT | face embedding caches over 1 GiB are read record by record | (r6 PR) |
| 3241 | feat(raw): integrate FlashRaw for high-performance camera RAW decoding | SKIP | new raw decoder dependency (FlashRaw); needs a licence/security review of a big dependency | (r6 PR) |
| 3242 | Reject EXIF offsets beyond 18 hours | ADOPT | EXIF time zone offset limited to 18:00 | (r6 PR) |
| 3243 | LR-VIEW-FILMSTRIP: a stack badge's tooltip names the stack toggle's key as bound (stacked on #2164) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3245 | LR-ENHANCE-DENOISE: research pretrained RAW denoising models and lice… | SKIP | research document | (r6 PR) |
| 3253 | LR-BEHAV-ACCESS: an icon button's accessible name is its label, with the shortcut as its description | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3255 | LR-BEHAV-LOCALE: Settings ▸ General ▸ Language can sync with the system again (stacked on #3146) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3256 | doc_path: keep the gvfs test path under cfg(not(windows)) | SKIP | fork already has this gvfs test layout | (r6 PR) |
| 3257 | LR-BEHAV-TEXTFIELD: emoji are in color on macOS | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3258 | Library lock: Only remove the owner note this lock wrote (#3240) | ADOPT | a library lock removes only the owner note it wrote (fork keeps read_no_follow) | (r6 PR) |
| 3259 | LR-BEHAV-ACCESS: a button that does something reads as a button and one of several choices as a radio button (stacked on #2078) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3260 | LR-BEHAV-ACCESS: a dialog is exposed to screen readers as a dialog, not a group (stacked on #2078) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3261 | LR-BEHAV-EMPTY-STATES: the grid's title, count and empty-view messages are read by screen readers (stacked on #2891) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3262 | LR-BEHAV-ACCESS: the photo in Detail is named with its file name (stacked on #2078) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3263 | LR-BEHAV-ACCESS: a photo thumbnail is a selectable cell, not a check box (stacked on #3216) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3265 | LR-BEHAV-ACCESS: the grid's Thumbnail size reads as a slider (stacked on #2078) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3266 | LRC-DEV-TONAL: consolidate tonal processing as V3 | SKIP | new tonal process version V3; needs process-version work and render comparison | (r6 PR) |
| 3267 | Reject ZIP entries with inconsistent decoded sizes | ADOPT | ZIP entries with inconsistent decoded sizes refused (geocode) | (r6 PR) |
| 3268 | LR-REM-CONTENTAWARE: generate masked replacements with ComfyUI | SKIP | ComfyUI integration feature; network/process spawn, not a fix | (r6 PR) |
| 3269 | Grant network access so update checks can reach GitHub | SKIP | upstream packaging/updater permission | (r6 PR) |
| 3273 | LR-BEHAV-ACCESS: panels and bars are named as landmarks and toolbars (stacked on #2078) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3274 | Toggle color labels with keyboard shortcuts | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3275 | Honor CARGO_BUILD_TARGET in FreeBSD packaging for aarch64 release builds | SKIP | upstream FreeBSD packaging | (r6 PR) |
| 3276 | LRC-LIB-FOLDERS: the home folder and Local's built-in places can't be renamed or moved | SKIP | UI/library folders feature in fork-owned panels | (r6 PR) |
| 3279 | LR-EDIT-DETAIL-DENOISE: AI Denoise for finished pictures (JPEG, PNG, TIFF …) with a model of their own | SKIP | AI denoise feature for finished pictures; large | (r6 PR) |
| 3281 | Develop exports from a source at least twice the output size | SKIP | export source level change (memory/time), overlaps 3304; fork's export path diverged | (r6 PR) |
| 3282 | segment: opt-in CUDA device for SAM 3 on Linux and Windows | SKIP | opt-in CUDA device; native dependency, unverifiable here | (r6 PR) |
| 3283 | Refuse a signed or malformed Content-Length in HTTP responses | ADOPT | signed or malformed Content-Length refused (fetch) | (r6 PR) |
| 3284 | Issue #3250: choose how many of a person's faces a name suggestion is based on | SKIP | face feature, UI settings | (r6 PR) |
| 3285 | Issue #3246: choose the face that shows a person in People | SKIP | face feature, UI | (r6 PR) |
| 3286 | FACES: a person's page finds its look-alikes in seconds, not minutes | SKIP | faces feature performance in fork-owned UI path; needs run | (r6 PR) |
| 3287 | LR-BEHAV-BG-TASKS: an Auto Import taken out of the waiting line gives its files back (stacked on #1943) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3288 | MENU-VIEW-SORT: the Sort menus offer only what the view uses | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3289 | LR-FILT-SORT: a new sort order or reshuffle keeps the active photo in view | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3290 | LR-FILT-SORT: a shuffled grid says so in its header, with Reshuffle beside it | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3291 | LR-BEHAV-LOCALIZE: the translation report keeps same-named helpers of different files apart and reads a slider's text (stacked on #2907) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3294 | LR-FILT-SORT: Previous Shuffle and a Reshuffle shortcut (stacked on #3288) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
| 3295 | MENU-BAR: merge the native menu's identical quit/about/settings branches (clippy::if_same_then_else) | SKIP | clippy-only | (r6 PR) |
| 3296 | Issue #3293: README no longer lists an MCP mask helper | SKIP | not immediately valuable to fork users | (r6 PR) |
| 3297 | Issue #3251: People card badge shows the full photo count | SKIP | UI badge | (r6 PR) |
| 3298 | Issue #3272: AI masks are enlarged along the photo's edges, no outline under a raised Exposure | SKIP | large pipeline mask refinement (guided filter in the render path); perf and look need a measured run | (r6 PR) |
| 3301 | CI: simplify the detail prefetch texture guard | SKIP | clippy-only | (r6 PR) |
| 3304 | #3270: exports of preview size develop the full sensor | SKIP | export source level change (memory/time), overlaps 3281; fork's export path diverged | (r6 PR) |
| 3305 | ci(deps): bump storytold/craft-common-actions/.github/workflows/windows-arm64.yml from 8bb4e5ab95443171104ad88113fe1ef9b0e5ce3f to fa5ef18780cf5e7d5d3b8de068de56cffc83cac5 in the actions group | SKIP | upstream CI dependency bump | (r6 PR) |
| 3306 | LR-FILT-SORT: the header's Reshuffle reads as a button beside the Shuffled label (stacked on #3290) | SKIP | UI/menu/accessibility change in fork-owned ui-egui or native menu; fork redesigned these | (r6 PR) |
