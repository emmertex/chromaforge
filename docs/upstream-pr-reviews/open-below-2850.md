# Review of open upstream PRs numbered below 2850

Reviewed 2026-10-26: every open pull request on upstream (the `upstream` remote) numbered below 2850, per the
upstream policy in `docs/upstream-merge.md` (port bug, security, crash, raw/codec and engine fixes; skip UI reworks,
label renames, upstream CI/packaging/docs and dependency bumps).

- **ADOPT**: ported as is (resolved against the fork where needed), with the fork PR that landed it.
- **LEARN**: not taken as is, but the idea or fix is worth implementing here; *own take* rows were done in a fork PR, the others are follow-ups.
- **SKIP**: not useful to the fork, already fixed here, or out of scope.

Totals: 36 adopted, 75 learn, 125 skipped (236 PRs).

| PR | Title | Decision | Reason | Fork PR |
|---|---|---|---|---|
| #2849 | LR-EXP-METADATA: refuse Originals with embedded place names | LEARN | refuse Original export with embedded place names when location removal is on: privacy fix worth porting | fork #524 |
| #2848 | LRC-SS-LAYOUT: the Slideshow settings page no longer moves when the slide preview finishes rendering | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2844 | GPU: black pixels take the colour tools and a light vignette as on the CPU | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2841 | Write sidecar keywords as names in dc:subject and paths in lr:hierarchicalSubject | LEARN | keywords as names in dc:subject, paths in lr:hierarchicalSubject: interop fix worth checking against the fork's sidecar writer || fork #536 |
| #2839 | Docking: side panels in dock columns, on craft-ui's shared docking | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2838 | Lightroom import: keep compressed history-step and snapshot settings in the archive | LEARN | Lightroom catalog import keeps compressed history-step and snapshot settings |  |
| #2835 | raw: decode lossy single-plane (grey) JPEG XL LinearRaw DNGs (Lightroom monochrome merges) | LEARN | grey lossy JPEG XL LinearRaw DNGs (Lightroom monochrome merges): fork jxl.rs diverged; worth re-implementing | fork #529 |
| #2834 | Clamp negative signals to zero in hlg_decode | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2833 | Strip trailing NULs from non-UTF-8 IPTC text | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2831 | Docs: link the lens profile gap and roadmap priority to #329 | SKIP | upstream docs/roadmap only |  |
| #2830 | LR-BEHAV-MULTISELECT: replacing the selection with a repeated id keeps each photo once | LEARN | repeated id in a selection replacement kept once: tiny engine fix || fork #536 |
| #2828 | LRC-LIB-SMARTCOLL: reorder rules and groups in the smart album editor by drag or arrow keys | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2827 | GPU: compute preview histograms and result diagnostics with compact reductions | LEARN | GPU histograms with compact reductions: perf idea for the fork's GPU path |  |
| #2825 | LR-LIB-ALBUM: album commands skip photos the library lacks and record nothing when they add nothing | LEARN | album commands skip unknown photos and record nothing when no-op || fork #536 |
| #2823 | LR-BEHAV-DRAGDROP: a right-click on a text field while photos are carried opens no menu | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2822 | LR-VID-PLAY: MP4/MOV video import and cataloguing with duration and container badges | LEARN | MP4/MOV import and cataloguing: compare with the fork's lcc-video |  |
| #2821 | LR-META-LOCATION: reject malformed typed GPS coordinates | LEARN | reject malformed typed GPS coordinates: small engine validation worth porting | fork #524 |
| #2818 | LRC-LIB-SMARTCOLL: a smart album on a loop of albums holds no photos, and says which album it loops through | LEARN | smart album on a loop of albums holds no photos: same family as #2798 | not taken: overlaps #2798 (now refused at edit time); the fork already bounds loops in damaged catalogs |
| #2812 | PERF-CATALOG: equal develop settings are held once in memory | LEARN | equal develop settings held once in memory: catalog memory win worth measuring || not taken: the fork's v4 store already shares equal settings on load (settings_ref) |
| #2809 | LR-CROP-AUTO: Auto straighten reads the vertical vanishing point and the horizon | LEARN | Auto straighten from vertical vanishing point and horizon: pipeline feature worth evaluating |  |
| #2808 | LR-BEHAV-DRAGDROP: a right-click while photos are carried opens no context menu | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2806 | LR-BEHAV-BG-TASKS: a preview build asked for while another runs can wait in line for it (engine) | LEARN | preview builds queue behind a running one (engine) |  |
| #2805 | Enable Filters: a folder moved while filters are off is followed by the filter put aside, and Cmd+L still brings it back (follow-up to #2709) | LEARN | Enable Filters follows a moved folder: check against the fork's filter model |  |
| #2804 | fix(ui): wrap the Edit header buttons instead of letting Reset cover HDR | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2803 | chore(deps): bump the cargo-dependencies group across 1 directory with 19 updates | SKIP | dependabot: we manage our own dependencies |  |
| #2802 | Add an Android app shell | SKIP | the fork has its own Android port (own design) |  |
| #2798 | LR-LIB-SMARTALBUM: the catalog refuses an edit that makes a smart album include itself | LEARN | catalog refuses a smart album that includes itself: worth porting to the fork's catalog | fork #525 |
| #2797 | LR-BEHAV-AUTOSAVE: a change that could only be partly taken back is saved and can be undone | LEARN | autosave of partly-undoable changes: idea for the fork's undo model |  |
| #2796 | docs: gap 9 — the CI gate runs the tests on Linux x64; macOS, Windows and the GPU tests still don't | SKIP | upstream docs/roadmap only |  |
| #2789 | Grading lgg vectorscope | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2787 | LR-IMP-FORMATS: decode GoPro GPR (VC-5) | LEARN | GoPro GPR (VC-5) decoder: raw format support worth porting || fork #531 |
| #2781 | LR-LIB-VIRTUALCOPY: prevent numbering overflow in copy batches | LEARN | virtual-copy numbering overflow: small never-crash fix, conflicts; port by hand | fork #524 |
| #2780 | security/raw: bound hostile-memory paths in the raw decoders | LEARN | raw hostile-memory bounds (SonarCloud): good findings, but conflicts with the fork's reworked tiffraw/mef/exif; port per finding later | fork #522 |
| #2777 | LR-EDIT-OPTICS-PROFILE: Fujifilm X-S20 embedded lens corrections and spectral matrices | LEARN | Fujifilm X-S20 embedded lens corrections and spectral matrices: camera support | fork #529 |
| #2776 | Reject signed numeric character references in XML entity decoding | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2772 | Logging: each render request and how it ended, at debug level (#2682) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2771 | Read Canon maker-note lens when LensModel is missing | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2770 | security: bound heic-rs 0.1.4 work around hostile HEIF containers | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2768 | LR-VIEW-FILMSTRIP: a selected photo's ring shows before its thumbnail loads | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2766 | LR-BEHAV-DRAGDROP: only the primary button drags and drops photos from the grid | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2765 | LR-VIEW-PHOTOGRID: the grid asks the image decoders for no thumbnail of a video (#2682) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2764 | LR-VIEW-PHOTOGRID: a thumbnail that can't be read is tried again only when its photo changes (#2682) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2759 | KEYC-RATING: Enable Filters brings a filter back without its album when an album is shown | LEARN | Enable Filters brings back a filter without its album |  |
| #2758 | KEYC-RATING: deleting the only keyword of the filter Enable Filters put aside leaves filtering on | LEARN | deleting the only filter keyword leaves filtering on |  |
| #2757 | LR-BEHAV-MULTISELECT: several sidebar albums, keywords or folders are deleted, moved or labeled as one undo step (engine) | LEARN | multi-row sidebar ops as one undo step (engine): idea |  |
| #2756 | LR-BEHAV-SIDEBAR-SECTIONS: Option-click on a triangle opens or folds the whole branch | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2753 | Reject truncated times of day in normalize_iso instead of using midnight | ADOPT | normalize_iso refuses truncated times; done in dates.rs, the fork keeps iso_seconds lenient for sorting | fork #510 |
| #2752 | LR-BEHAV-DRAGDROP: a Folders row dragged onto another moves that folder on disk, after a question | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2751 | LRC-LIB-LIBFOLDERS: several folders move into one in a single undo step (folder.move paths) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2750 | Never crash: tiles far larger than the image are refused before decoding (#2554) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2749 | LRC-LIB-LIBFOLDERS: after moving a folder, the message says where it went, in every language | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2748 | LR-CLOUD-SMARTPREVIEW: the smart-previews folder test cleans up the folder it takes offline | ADOPT | safety / correctness fix in engine, develop or catalog | fork #513 |
| #2747 | LRC-LIB-LIBFOLDERS: moving a folder refuses a missing destination and says why in plain words | SKIP | feature or rework outside the policy's port list (or too large / tied to upstream structure) |  |
| #2746 | Tests: fix the Windows clippy failure in doc_path's GVfs check (unused variable since #2412) | SKIP | already in the fork (applies as a no-op) |  |
| #2743 | LR-MERGE-PANO: the merge dialog makes a new preview each time it opens, also after a failed one (#2708) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2742 | LRC-LIB-CATALOG-SWITCH: opening a library from the menus closes the menu and says which library is opening (#2718) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2741 | LR-VIEW-HISTOGRAM: the histogram stays in view while the Edit panel's sliders scroll (#2679) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2740 | Never crash: ARW and SRW strips shorter than declared are refused before allocating (#2561) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2738 | Never crash: Refuse an ORF whose 12-bit strip runs past the file (#2561) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2736 | LR-BEHAV-DRAGDROP: the message after a drop that moved, added or tagged photos has an Undo button | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2735 | LR-BEHAV-DRAGDROP: a drop's destination is named with its parent when another folder or album has the same name | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2734 | LR-BEHAV-DRAGDROP: the photos being dragged are dimmed in the grid | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2733 | LR-EDIT-OPTICS-PROFILE: Sigma 10-18 mm exception on the ILCE-6700 (stacked on #2732) | LEARN | Sigma 10-18 on ILCE-6700 (stacked on #2732) | fork #529 |
| #2732 | LR-EDIT-OPTICS-PROFILE: Sony ILCE-6700 in-file distortion | LEARN | Sony ILCE-6700 in-file distortion: camera support | already in the fork (ILCE-6700 in-file distortion) |
| #2731 | Brush dabs: build only as far as the caller will look (#1520) | ADOPT | safety / correctness fix in engine, develop or catalog | fork #513 |
| #2726 | LR-BEHAV-LOCALIZE: panel layout toasts, face recognition prompts and the second window title are translated | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2725 | LRC-LIB-CATALOG-LOCK: quitting on macOS removes the lock's owner note | ADOPT | safety / correctness fix in engine, develop or catalog | fork #513 |
| #2724 | fix(codecs): validate JPEG ICC APP2 chunk sequences | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2723 | Control port: close gracefully so the client reads its last reply (Windows reset) | SKIP | graceful control-port close already in the fork |  |
| #2722 | LR-VIEW-FILMSTRIP: photos drag from the filmstrip like from the grid | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2721 | LRC-LIB-LIBFOLDERS: a refused move to another folder says what is wrong in plain words; photo.checkMove and skip | SKIP | feature or rework outside the policy's port list (or too large / tied to upstream structure) |  |
| #2717 | Never crash: rational tag lists show 16 values like every other list (#2579 item 7) | ADOPT | rational tag lists capped like other lists (merged into the fork's let-chain match) | fork #510 |
| #2715 | LR-BEHAV-ACCESS: every color pair the UI paints stands out in every theme (today's gaps listed) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2714 | Never crash: History is bounded on load and by every op that sets it (#2579 item 8) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2712 | Never crash: NaN in tone curves and an i16::MIN UTC offset (#2579 items 3 and 4) | ADOPT | i16::MIN UTC offset part taken; tone-curve NaN half was already fixed here | fork #510 |
| #2707 | LR-BEHAV-MULTISELECT: the model for choosing several sidebar rows with Cmd/Ctrl+click and Shift+click | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2706 | M12-OPEN: AI masks on openly licensed models in pure Rust | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2705 | LR-IMP-FORMATS: a JPEG cut short imports from the data present | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2704 | LR-BEHAV-ACCESS: an unnamed control on a main screen fails the tests (today's gaps listed) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2703 | LR-BEHAV-LOCALIZE: the translation report also lists plain tr() strings a catalog lacks, wrapped calls included | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2698 | LR-IMP-FORMATS: AHD blends its candidates where noise alone would pick one, no maze in noisy raws (#2094) | LEARN | AHD blends candidates in noise (no maze in noisy raws): demosaic quality idea |  |
| #2697 | LRC-LIB-FOLDERS: Local says where its listing stops, with a Go Deeper row | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2696 | LRC-LIB-LIBFOLDERS: a count never covers a name in a narrow sidebar | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2693 | "Favorite" and "Listing folders" in the catalogs that lack them | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2691 | LR-PREF-GENERAL: Esc in a Settings text box gives the text back instead of closing Settings | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2676 | LR-EDIT-OPTICS-PROFILE: preserve unknown crop factors in lens profiles | LEARN | keep unknown crop factors in lens profiles || fork #536 |
| #2675 | LR-PROF-CAMERACOLOR: align lens-corrected camera JPEG training pairs | LEARN | align lens-corrected camera JPEG training pairs |  |
| #2664 | raw: read Canon CR3/CR2 preview colour space from the maker note (#2654) | LEARN | CR3/CR2 preview colour space from the maker note: conflicts with the fork's camera_preview.rs; worth porting by hand | fork #529 |
| #2661 | LR-EDIT-LENSBLUR: camera-like Lens Blur with AI subject focus and per-mask bokeh | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2660 | LR-PROF-IMPORT: export a look as a 3D LUT | LEARN | export a look as a 3D LUT: feature idea |  |
| #2659 | LR-META-LOCATION: place names always download into the settings folder (#2575) | ADOPT | place names only download into the settings folder; geonames links removed before the fork's write-through atomic write | fork #513 |
| #2658 | LC-SAFETY: Refuse to create a library photo's XMP sidecar | ADOPT | safety / correctness fix in engine, develop or catalog | fork #513 |
| #2656 | LR-PROF-CAMERACOLOR: limit tone-curve steps between pixel populations (#1904) | LEARN | limit tone-curve steps between pixel populations (camera colour fitting) |  |
| #2653 | Detail view: zoom to 1600 % (#2652) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2651 | LR-FILT-FLAG: the filter bar's flag buttons combine, Picked + Unflagged hides only the rejects | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2650 | LR-EXP-DIM: export resampling filters | LEARN | export resampling filters: feature idea |  |
| #2649 | LR-LIB-DELETE: the catalog answers "which photos have a file in this folder / of this name" from an index | LEARN | catalog index for files by folder/name: perf idea for the fork's catalog || not taken: the fork's Delete from Disk plan is one pass over the library already |
| #2648 | Never crash: the work gate's byte count saturates instead of overflowing (#2576) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2647 | LR-LIB-SMARTALBUM: editing a smart album made from an album view keeps the album | LEARN | editing a smart album made from an album view keeps the album |  |
| #2645 | Detail view: zoom past 100% shows the photo's pixels instead of a bilinear blur (#2643) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2644 | feat: bundled Canon EF/RF lens profiles, monotone cubic camera tone curves, and filmstrip drag throttling | LEARN | bundled Canon lens profiles, monotone cubic tone curves: large, check asset licences before taking any part |  |
| #2641 | LR-EXP-DIALOG: the Export dialog updates, renames, duplicates and deletes a loaded preset (#2510) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2640 | LRC-LIB-PEOPLE: the People view follows the folder or album shown in the sidebar (#2516) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2639 | LR-MASK-BRUSH: with the mask overlay hidden, the stroke being painted no longer tints the photo (#2549) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2638 | LR-EXP-PSD: Edit In starts only the editor set in Settings (#2551) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2637 | Catalog: library files are never read, cut or appended through a planted link (#2552) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2636 | Never crash: a keyword nested thousands of levels deep keeps its top 64 levels (#2558) | SKIP | keyword nesting already bounded here (MAX_TREE_LEVELS) |  |
| #2635 | Never crash: lossy-JPEG tiles and embedded previews are refused from their JPEG header (#2560) | LEARN | tile path now uses the fork's existing header-preflighted lossy_jpeg_limited with an MCU-based cap (own take, upstream code didn't fit) | fork #510 |
| #2634 | LR-MASK-EFFECTS: mask.exportEffects refuses originals and writes atomically (#2564) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2633 | Desktop: the panic log is kept in the settings folder, never the shared temp folder (#2572) | SKIP | feature or rework outside the policy's port list (or too large / tied to upstream structure) |  |
| #2632 | MCP: render and screenshot PNGs go through a private temp folder (#2573) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2631 | LR-EDIT-DETAIL-DENOISE: GPU denoise doesn't hang when its tiles in flight fail (#2577) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2629 | LRC-LIB-MISSING: library.missing from the control channel checks the files off the UI thread | LEARN | library.missing off the UI thread from the control channel |  |
| #2628 | raw: CR3 preview box scan checks 32-bit lengths for overflow (#2569) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2627 | #1952: the idle-close MCP test reads blocking and waits for the close | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2624 | i18n(it): translate the effect presets, slideshow, print, people and overlay strings | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2623 | LR-CROP-RECT: sidecar settings kept in range; a turned crop bounded to the photo (#2555) | ADOPT | safety / correctness fix in engine, develop or catalog | fork #513 |
| #2615 | FreeBSD: package the aarch64 cross build (craft-common-actions#39) | SKIP | upstream CI/packaging/tooling |  |
| #2607 | LR-IMP-FORMATS: GIF and BMP sizes from the header; a canvas the file can't fill is refused (#2553) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2605 | Write capture time UTC offset and sub-seconds in write_exif | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2600 | LR-EXP-LOCATION: an export subfolder never leaves the destination folder (#2571) | SKIP | the fork already refuses hostile export subfolders (#80) more strictly; ported then reverted |  |
| #2599 | LRC-LIB-CATALOG-IMPORT: import Lightroom catalog backup zips directly (#2592) | LEARN | import Lightroom catalog backup zips directly |  |
| #2598 | Colour NR: luminance-weighted blur keeps channel means (part of #840) | LEARN | colour NR keeps channel means (CPU+GPU); conflicts with the fork's local.rs/map.wgsl, needs a measured port || already in the fork: colour NR blurs luminance-weighted rgb (local::colour_nr) |
| #2597 | LRC-LIB-LIBFOLDERS: the folders test fixture waits for the Missing Photos row | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2587 | fix(camera-preview): prefer better accepted per-photo colour fits | LEARN | prefer better accepted per-photo colour fits |  |
| #2584 | i18n(sv): translate 1074 new strings, use the agreed terms in 16 | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2583 | Reset the profile section to the default profile | ADOPT | Profile section reset restores the default profile (fork lacked it) | fork #513 |
| #2582 | i18n(ka): Georgian for 336 interface strings, 48 format messages and the 559 untranslated What's New lines | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2581 | LR-META-XMP: Read referenced JPEG Extended XMP | LEARN | Extended XMP GUID/coverage validation and GPS refusal on Original export; fork already reassembles extended XMP, add the validation || own take in fork #533 (only the referenced, complete Extended XMP packet is read) |
| #2547 | Tests: discovery tests use the platform's path separator (Windows) | SKIP | already fixed in the fork (fork PR #409) |  |
| #2545 | Issue #136: applying the GPU setting never waits for the device on the UI thread | LEARN | GPU setting never blocks the UI thread waiting for the device |  |
| #2544 | LR-EXPORT: simplify preset bit-depth guard | ADOPT | safety / correctness fix in engine, develop or catalog | fork #513 |
| #2540 | Reject non-finite values when parsing .cube LUTs | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2537 | M12: SAM 3 mask decoder on the GPU, and the docs (5/5 of #2167) | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2536 | M12: SAM 3 DETR encoder on the GPU (4/5 of #2167) | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2535 | M12: SAM 3 necks on the GPU (3/5 of #2167) | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2534 | M12: SAM 3 image encoder on the GPU via wgpu (2/5 of #2167) | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2533 | M12: SAM 3 accelerator plumbing (1/5 of #2167): backend choice, CPU fallback and the AI masks setting | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2531 | LR-EDIT-CURVE: show the selected Tone Curve's sampled input histogram (#987) | SKIP | upstream docs/roadmap only |  |
| #2530 | Return None from shift_iso for impossible calendar dates | ADOPT | shift_iso refused impossible calendar dates (the fork only checked normalize_iso) | fork #510 |
| #2529 | LR-MAP-TRACKLOG: Decode XML character references in GPX | ADOPT | safety / correctness fix in engine, develop or catalog | fork #513 |
| #2527 | Kelvin white balance for more raws without colour matrices (#730, depends on #2514) | LEARN | Kelvin WB for raws without colour matrices (depends on #2514) |  |
| #2525 | LR-PREF-GENERAL: what Settings says can be read in every theme, and in every language it has | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2524 | LR-PREF-GENERAL: the Settings tabs are grouped by what a person comes to do | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2523 | LR-PREF-GENERAL: Settings asks before Clear Cache, a model's Remove and deleting smart previews | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2522 | LR-PREF-GENERAL: Settings tabs look like tabs, and the keyboard focus shows and moves in one short way | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2521 | LR-VIEW-ZOOM: test pinch display fitting with controlled completions | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2520 | i18n: add Vietnamese (vi) interface language | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2514 | Kelvin for Fujifilm X-Trans III bodies from the RAF's own neutral pair (#730) | LEARN | Kelvin for X-Trans III from the RAF neutral pair |  |
| #2512 | raw: decode Canon CRW (CIFF) files | SKIP | the fork already decodes CRW (vendor/crw.rs) |  |
| #2508 | <app>-web: postMessage host bridge (`?host=parent`) | SKIP | upstream web shell change; the fork's web app differs |  |
| #2495 | LR-IMP-FORMATS: decode Sony ARW version 1 (DSLR-A200 to A390) | SKIP | the fork already decodes ARW v1 (vendor/arw1.rs) |  |
| #2492 | LR-MERGE-PANO: report omitted photos and pair verification outcomes | LEARN | pano merge reports omitted photos and pair verification |  |
| #2491 | feat(linux): export the existing menubar to KDE Plasma Global Menu | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2490 | Convert Negative 1/4: <app>-film, the negative conversion maths | SKIP | feature or rework outside the policy's port list (or too large / tied to upstream structure) |  |
| #2487 | Light LRI (L16): decode one camera module with its in-file calibration | LEARN | Light L16 LRI decoder: niche raw format |  |
| #2485 | catalog.query / library.filter: an unknown filter key is an error, not a silent whole-library match (#2241) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2483 | LR-MERGE-HDR: sharp handheld HDR merges — refined and local alignment, the reference's finest detail (3/3 of #1887) | LEARN | sharper handheld HDR merges: merge quality idea |  |
| #2479 | Local: work out added photos' content hashes on a worker thread (2/4 of #2001) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2478 | Local: show a browsed folder's files as placeholders while they are read (3/4 of #2001) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2477 | Local: read only a browsed raw's headers; hash it when it joins the library (1/4 of #2001) | LEARN | Local: read only a browsed raw's headers; hash on join: perf idea |  |
| #2465 | TIFF: Read every plane of a planar TIFF (#2462) | ADOPT | never-crash / hostile-input / correctness fix in a shared crate | fork #510 |
| #2464 | LRC-LIB-MISSING: one rule for Missing Photos' files, and a folder listing that panics no longer stays running | LEARN | one rule for Missing Photos and a panicking folder listing no longer stays running |  |
| #2463 | LR-VIEW-HQPREVIEW: View ▸ High Quality Preview | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2459 | PERF-WALK: a folder walk on a network share lists up to eight folders at the same time | LEARN | parallel folder walk on network shares: perf idea |  |
| #2445 | LR-PROF-CAMERA: measured looks for the X-T4's film simulations | LEARN | measured X-T4 film-simulation looks |  |
| #2441 | feat(ui): add "Show in Folders" to the photo menu (#2437) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2439 | LR-PREF-DISPLAY: on Windows, previews follow the monitor's colour profile | LEARN | Windows previews follow the monitor colour profile: feature idea (adds a crate) |  |
| #2435 | Add Korean support | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2434 | LR-PREF-PERFORMANCE: percentage and pixel limits for previews (#1897) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2429 | i18n: complete the Traditional Chinese (zh-hant) catalog (200 missing strings) | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2427 | i18n: complete the Simplified Chinese (zh-hans) catalog (162 missing strings) | SKIP | upstream locale catalogs; the fork keeps its own overlay |  |
| #2423 | PERF-SMART: test a smart album's rules cheapest first | LEARN | smart-album rules tested cheapest first: perf idea || deferred: reordering needs a loop-aware guard; low value after the fork's PERF-SMART tally |
| #2421 | fix(windows): add opt-in DirectComposition workaround for VRR flicker | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2419 | PERF-SIDECARS: imports and moves answer sidecar lookups from the folder listing | LEARN | sidecar lookups from the folder listing: perf idea || own take in fork #535 (imports; rename/move lookups not changed) |
| #2418 | PERF-SEARCH: typing in the library search keeps up on a large library | LEARN | library search keeps up on large libraries: perf idea (catalog) || own take in fork #534 |
| #2415 | PERF-LIB: big views list from a kept sort order instead of sorting every photo | LEARN | big views from a kept sort order: perf idea || not taken: the fork already keeps sort orders (query::SortCache) |
| #2414 | PERF-SELECT: a big selection no longer slows every idle frame | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2395 | web: ship the craft-fonts faces beside the WASM so the web release can use craft-fonts | SKIP | upstream CI/packaging/tooling |  |
| #2394 | Tone Curve: put a clicked new point on the curve | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2390 | #2359: Remove is content-aware, as in Lightroom (stacked on #2367) | LEARN | content-aware Remove (stacked on #2367) |  |
| #2388 | LR-LIB-CULL: Assisted Culling measures photos on all cores (stacked on #2283) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2384 | Grid: Outline the selection in dark on the light themes (#2382) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2381 | #2284: date files and the session clock in the host's time zone | LEARN | host-time-zone dates and session clock |  |
| #2378 | LR-BEHAV-BG-TASKS: detached readers for an import or scan (stall, skip, cap), tested and not used yet | LEARN | detached readers for import/scan: infrastructure, unused upstream yet |  |
| #2373 | LR-BEHAV-ACCESS: Reduce motion stops zoom, scroll and spinner animations, and follows macOS until you choose | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2370 | LR-BEHAV-TOAST: toasts stay long enough to read, hold under the pointer, and errors stay 6 s | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2368 | #2359: Remove tool: one outline per brushed spot, strokes at a spacing, brush sized by the mouse | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2367 | #2359: content-aware Fill keeps structure and grain, nothing ghosts through the feather | LEARN | content-aware fill keeps structure and grain: healing quality |  |
| #2358 | LR-MASK-PEOPLE: People masks with SAM 3: pick a face, then Entire Person / Face Skin / Body Skin / Hair / Clothes (#2357, step 1) | SKIP | large AI-mask stack (thousands of lines across fork-owned UI); revisit as a whole when it lands upstream |  |
| #2355 | LR-EDIT-DETAIL-DENOISE: Fujifilm X-Trans support via LinearRgb contract with GPU acceleration | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2351 | gpu, windows: fix DX12 readback polling on RTX Spark (Windows ARM64) and Windows test paths | LEARN | DX12 readback polling with submission index (Windows ARM64): GPU robustness idea |  |
| #2350 | LR-LIB-CULL: arrange bursts by hand with drag and drop, the first photo is the pick (stacked on #2281) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2344 | feat(crop): Lightroom-style canvas and straighten band (opt-in; Photoshop stays default) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2340 | Process V3: a sharp resample, and a sharp fit-to-screen and zoomed view | LEARN | Process V3 sharp resample / fit view: render-fidelity idea || stopped: depends on 2332 (sharp resample / fit view); needs a measured port |
| #2338 | feat(ui): Lightroom Classic keymap profile (opt-in, user bindings stay on top) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2332 | Process V3: sharpening at the original's pixels and a raw base look (LR-BEHAV-RENDER-FIDELITY) | LEARN | Process V3 sharpening at the original pixels: render-fidelity idea || stopped: overlaps the fork's Process V4 sharpening; needs a measured port as a new fork process |
| #2330 | LR-EDIT-EFFECTS-GRAIN: replace grid noise with signed particle grain | LEARN | signed particle grain: effect quality idea |  |
| #2329 | Fill missing GPS locations for the current view | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2328 | #2311: Linear and Radial Gradients are drawn by dragging on the photo | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2326 | Search embedded metadata using a rebuildable cache | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2325 | LR-VIEW-SECONDWINDOW: the second window's own zoom | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2322 | LR-CROP-RECT: a free crop corner on a straightened photo grows both sides (#2308) | SKIP | feature or rework outside the policy's port list (or too large / tied to upstream structure) |  |
| #2316 | #2063: decode pre-X FinePix and SuperCCD RAFs | SKIP | the fork already decodes pre-X / SuperCCD RAFs (raf_superccd.rs) |  |
| #2315 | #2270: a host bridge for pages framing the web build (?host=parent) | SKIP | upstream web shell change; the fork's web app differs |  |
| #2313 | LR-BEHAV-RENDER-FIDELITY: process V3 Highlights, Shadows, Whites and Blacks (#523) | LEARN | Process V3 Highlights/Shadows/Whites/Blacks: render-fidelity idea || stopped: process-version render-fidelity work tuned on upstream's own process numbering and Lightroom references; needs a measured port as a new fork process |
| #2310 | Import JPE/JFIF JPEGs and preserve mixed-case XMP sidecars | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2305 | Group unnamed faces and improve face assignment review | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2303 | xtask warm: start a new worktree's target/ from a clone of the main checkout's | SKIP | upstream CI/packaging/tooling |  |
| #2290 | #2284: read the Exif and XMP items of HEIF files | SKIP | HEIF Exif/XMP items already read by the fork (meta::heif) |  |
| #2289 | Rate, flag and label every photo of a collapsed stack | LEARN | rate/flag/label all photos of a collapsed stack: fork library.rs diverged; re-implement mark_targets | fork #525 |
| #2287 | LR-CROP-DIMMING: adjustable crop-mask opacity | SKIP | upstream docs/roadmap only |  |
| #2283 | LR-LIB-CULL: Assisted Culling measures focus on the subject — face regions, focus point (#2276, stacked on #2279) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2281 | LR-LIB-CULL: Assisted Culling can stack each burst, best photo on top (#2277, stacked on #2279) | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2278 | LR-PROF-CAMERACOLOR: raw decoding keeps negative channels | LEARN | raw decoding keeps negative channels: colour accuracy idea, needs measuring |  |
| #2259 | LR-BEHAV-BG-TASKS: notifications under the bell, import warnings in a report (stacked on #1642, review only the commits after 83c733e9) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2250 | Detail: The Before view waits for its own render instead of flashing the edits (#2248) | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2246 | Lightroom catalog import: turn masks into the photo's upright frame | LEARN | Lightroom catalog import: masks into the upright frame |  |
| #2243 | Library: Refuse an unknown filter key in catalog.query and library.filter (#2241) | SKIP | duplicate of #2485 (ported) |  |
| #2237 | Import: add a remembered thumbnail-size slider | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2227 | web: build the Import picker's accept list from the engine's EXTENSIONS | SKIP | upstream web shell change; the fork's web app differs |  |
| #2222 | LR-PROF-CAMERACOLOR-CHOICE: Calibrated camera colour, a per-photo choice of a raw's starting colour | LEARN | per-photo calibrated camera colour choice |  |
| #2218 | Native menu: Send Cmd+A to the focused text field, not Select All | SKIP | feature or rework outside the policy's port list (or too large / tied to upstream structure) |  |
| #2211 | LR-IMP-DUPES: speed up folder imports with header reads and on-demand hashing | SKIP | upstream CI/packaging/tooling |  |
| #2209 | Export: built-in presets for Instagram Feed, Story / Reel, Web / Portfolio, Full Resolution (#2104) | LEARN | built-in export presets for social/web: feature idea |  |
| #2207 | Presets change only the settings they name (fixes #2159) | LEARN | presets change only the settings they name: behaviour fix worth checking |  |
| #2206 | ui: first-run onboarding tour and What's New after an update | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2204 | LR-BEHAV-DRAGDROP: photo drags say what a drop will do, refuse what it can't, and Esc gives them up | SKIP | UI-only change; ui-egui is fork-owned (Modern layout) and UI reworks are skipped per the upstream policy |  |
| #2203 | catalog: history steps and versions share segmentation grids (#2154) | LEARN | history steps and versions share segmentation grids: memory idea |  |
| #2202 | Negative Craft: turn a scanned film negative into a normal photo | SKIP | upstream docs/roadmap only |  |
| #2194 | MENU-BAR: every command has a place in the menus, and View drops from 62 rows to 36 | SKIP | mostly UI (fork-owned ui-egui) with engine plumbing tied to it; low value against the fork's layout |  |
| #2188 | LR-PROF-CAMERACOLOR: smooth fitted tone curves; DRO lowering follows Sony's curve between knots | LEARN | smoothed fitted tone curves; DRO following Sony's curve |  |
| #2179 | catalog: resync finds the retry record from the end, not every marker (#2124) | SKIP | journal resync from the end already ported here (upstream #2153) |  |
| #2176 | feat(mcp): confine file access to configured roots | LEARN | MCP file access confined to configured roots: security idea for the fork's MCP |  |
| #2174 | Twenty-five small fixes: NEF bit depth, colour labels, planar TIFF, light-mode UI, masks, spiral, Show in Folders, window fit, web title, no-keyword filter, radial feather, preset groups, people sort, preset highlight, clipping overlay, base curve, capture time, dehaze, culling, grid shortcut | SKIP | upstream CI/packaging/tooling |  |
| #2170 | never-crash: deny the panic lints in every crate root; GPU asserts become fail paths (#2137) | LEARN | panic lints in every crate root, GPU asserts as fail paths: check the fork covers the same crates |  |

## Missed in the first pass (1907–2168)

Still-open upstream PRs in this range that the first pass left out, reviewed 2026-10-26.

| PR | Title | Decision | Reason | Fork PR |
|---|---|---|---|---|
| #1907 | LR-META-XMP: Write automatic XMP sidecars off the UI thread | LEARN | a background sidecar writer keeps saves off the UI thread; the fork still writes inline (no `sidecar_writer`) |  |
| #1910 | i18n: add Lao (lo) translation support | SKIP | a new locale needs Lao fonts in craft-fonts and a translator to maintain it; revisit on request |  |
| #1916 | LR-BEHAV-ACCESS: red-eye corrections by keyboard | LEARN | keyboard placement of red-eye spots: accessibility idea, the fork's red-eye tool differs |  |
| #1926 | sidebar and bottom bar icons from Tabler Icons | SKIP | the fork keeps its own code-drawn icon set and Classic look |  |
| #1928 | Restore font-free GNOME Wayland window controls | LEARN | Wayland CSD caption buttons on GNOME; check the fork's winit features against GNOME before porting |  |
| #1931 | Library: a cropped photo's thumbnail reads the source its crop needs | SKIP | already in the fork (`media.rs` sizes the source by the crop) |  |
| #1933 | LR-BEHAV-BG-TASKS: preview builds queue, failed tasks can be retried | SKIP | stacked on unmerged upstream PRs; the fork's activity stack differs |  |
| #1936 | LR-BEHAV-TOAST: toasts stack instead of replacing one another | LEARN | stacked toasts: worth doing on the fork's own toast code |  |
| #1938 | Files on an SMB share: imports, moves and sidecar backups use the fsync fallback | SKIP | already in the fork (fork #375, `crates/sysmem` fsync fallback) |  |
| #1953 | Process V3: bright saturated colours keep their saturation | LEARN | tone-mapping change that would need a new process version so existing edits don't move; not ported blind |  |
| #1958 | M12: SAM 3 on the CPU ~3x faster | LEARN | large `crates/segment` rewrite; take after the fork's SAM 3 worker settles (upstream #2191 already ported) |  |
| #1961 | Remember Windows window geometry through eframe | SKIP | the fork has its own window-geometry handling |  |
| #1962 | Print: network printers, Grayscale and queue paper through craft-libs | SKIP | blocked on unmerged print-stack repos; the fork prints through `crates/winspool` |  |
| #1970 | Control port: require a per-run token before any request | ADOPT | already ported (commit 094401501) |  |
| #1971 | export EXIF Make and Model separately; camera label drops a repeated maker | LEARN | separate Make/Model on export is correct metadata; touches many crates, port by hand |  |
| #1973 | GPU: recover safely after readback timeouts | SKIP | already in the fork (`crates/gpu/src/ctx.rs` readback timeout recovery) |  |
| #1978 | Apple gain-map reconstruction and reference probes | LEARN | Apple legacy gain maps (linear ratio) for HEIC HDR; useful when the fork's HDR import reaches Apple files |  |
| #1995 | Import: preserve embedded rating, flag and label from file metadata | SKIP | the fork already takes rating, flag and label from embedded metadata on import |  |
| #2010 | LR-MERGE-HDRPANO: merge previews without the memory spike | ADOPT | already merged (commit 943d35a26) |  |
| #2018 | Workspace: rust-version 1.95 | SKIP | already in the fork (upstream #1859) |  |
| #2020 | LR-LIB-SMARTALBUM: keyword edits no longer freeze the window while albums are counted | LEARN | cached smart-album counts; check the fork's sidebar on a large library first |  |
| #2021 | Fix Lightroom catalog video classification and filtering | LEARN | video rows from a Lightroom catalog import; small, port with the next catalog-import work |  |
| #2029 | LR-MERGE-PANO: residual vignetting, flare offsets and seam leveling | LEARN | seam levelling for panoramas; sizeable, port with the next merge work |  |
| #2030 | LR-MERGE-PANO: merged DNGs open with sliders at 0 and their auto crop as default crop | LEARN | merged DNG opens neutral; port with #2029 |  |
| #2034 | LRC-LIB-LIBFOLDERS: Synchronize Folder can bring in files already in the library elsewhere | LEARN | duplicate handling in Synchronize Folder, remembered choice |  |
| #2043 | LRC-LIB-SMARTCOLL: Same Content as Another Photo rule | LEARN | smart-collection rule for twin files; small once #2034's duplicates exist |  |
| #2048 | CI: trigger platform checks for application changes | SKIP | upstream CI; the fork runs its own self-hosted CI |  |
| #2051 | LRC-LIB-CATALOG-IMPORT: header reads in fewer, larger reads | SKIP | stacked on unmerged upstream #1735 |  |
| #2060 | Wire to CollabCraft for Open, Save, and Share | SKIP | cloud service the fork does not ship |  |
| #2065 | KEY-TOGGLEZOOM: hold Space over a tool to pan and click-zoom | ADOPT | already ported (commit 448d88d9d) |  |
| #2068 | raw(orf): decode lossless compressed Olympus ORF files | SKIP | the fork already decodes compressed ORF (`orf::decode_compressed`) |  |
| #2073 | LC-SAFETY: refuse Photo Merge inputs beyond the memory budget | LEARN | never-crash: budget check before decoding merge inputs; the fork has decoder budgets (fork #527) but not this planner check |  |
| #2074 | Cobalt profiles | SKIP | the fork has its own profile picker and DCP import |  |
| #2075 | LR-BEHAV-DIALOGS: dialog confirm buttons name their action | LEARN | wording idea (Export, Rename… instead of OK) for the fork's dialogs |  |
| #2078 | LR-BEHAV-ACCESS: one tick box, radio and toggle component | SKIP | upstream widget refactor across its UI; conflicts with the fork's Classic UI and the egui 0.37 question |  |
| #2079 | LR-BEHAV-ACCESS: action buttons share one Button component | SKIP | same as #2078 |  |
| #2080 | MENU-BAR: one menu row and shortcut component | SKIP | same as #2078 |  |
| #2086 | LR-META-XMP: hierarchical keywords saved as names and paths | SKIP | already in the fork (`sidecar.rs`: names in `dc:subject`, paths in `lr:hierarchicalSubject`) |  |
| #2087 | LR-IMP-FORMATS: a HEIC's EXIF is read, and a file-date guess never reaches a sidecar | ADOPT | sidecar part already ported (commit 1a12f8149); HEIC EXIF part LEARN |  |
| #2088 | LR-META-XMP: a sidecar's GPS wins over the camera's | SKIP | already in the fork (`OWNED_IF_STATED` GPS tags) |  |
| #2092 | LR-PROF-CAMERACOLOR: color and tone in pushed shadows | ADOPT | already ported (fork #258) |  |
| #2093 | LR-PROF-CAMERACOLOR: bundled Nikon D780, D750 and D90 camera profiles | ADOPT | already in the fork (`assets/camera-profiles/NIKON_D7*0.json`, `NIKON_D90.json`) |  |
| #2102 | Handle macOS native picker failures without aborting | LEARN | never-crash on macOS picker errors; the fork's dialog code differs, port by hand |  |
| #2110 | LR-BEHAV-LOCALIZE: Hebrew file and folder names get glyphs | LEARN | Hebrew fallback face through craft-fonts |  |
| #2111 | LR-MASK-CURVE: a Curve in every mask | ADOPT | already ported (commit a6e822b4c) |  |
| #2143 | MIDI controller input: TOML map to Develop sliders and commands | LEARN | MIDI surface over the command layer; worthwhile but needs a pure-Rust MIDI crate review |  |
| #2155 | Never crash: refuse an ERF or MEF whose raw strip runs past the file | ADOPT | already ported (commit 3737f8504) |  |
| #2160 | LR-PROF-CAMERACOLOR: bundled X20, X100T, X-T1, D700 and D70 profiles | ADOPT | fitted by the contributor with `calibrate` from their own raws and in-camera JPEGs, no Adobe data | fork #555 |
| #2161 | Never crash: cap the property names one XMP packet can produce | SKIP | the fork already caps flattened keys (`MAX_KEY_BYTES` in `meta/src/xmp.rs`) |  |
| #2162 | LR-VIEW-PHOTOGRID: the active photo's selection ring shows on light themes | LEARN | grid ring contrast on light themes; apply to the fork's grid colours |  |
| #2168 | LRC-LIB-CATALOG-IMPORT: import a creation saved inside a collection | ADOPT | fixes a whole catalog import failing on a print/book saved inside a collection | fork #551 |
