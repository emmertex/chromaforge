# Closed-but-unmerged upstream PRs

Every pull request on the upstream repository (see `NOTICE`; numbers below are its PR numbers) that was closed without being merged (Dependabot left out), reviewed
2026-10-26 for what the fork should adopt (port as is), learn from (re-implement the idea) or skip. Clean-room
objections raised upstream are not a reason to skip here; a change that already reached upstream main (and so the
fork) by another route, or that the fork solved itself, is skipped as a duplicate.

Summary: 7 adopted, 6 to learn from, 161 skipped, of 174 reviewed.

| PR | Title | Decision | Reason | Fork PR |
|---|---|---|---|---|
| #2852 | LR-FILT-SEARCH-META: >= and <= in fielded search tokens | SKIP | duplicate: the fork's search already reads >= / <= (catalog query.rs) |  |
| #2662 | tests: build expected paths part by part so they pass on Windows | SKIP | duplicate: already upstream via #2547, so in the fork |  |
| #2608 | Control port test: send the second request while the first reply is pending (#2538) | SKIP | duplicate: #2710 fixed the same control-port test race |  |
| #2528 | Control port: close gracefully so the peer reads the last reply (Windows reset) | SKIP | duplicate: already upstream via #2377, so in the fork |  |
| #2480 | Local: no freeze on a slow share; list subfolders in parallel (4/4 of #2001) | SKIP | duplicate: folded into #2459 upstream (parallel subfolder listing) |  |
| #2426 | test: fix folder-drop tests on Windows (widget ids use forward slashes) | SKIP | duplicate: folder-drop widget ids fixed by #1975 |  |
| #2416 | Persistently ignore and restore face detections (catalog format 10) | LEARN | persistent Ignore Faces (catalog format 10): not in the fork; needs catalog-format work on the fork's catalog || fork #526 |
| #2377 | Ci/cross platform tests | SKIP | duplicate: already upstream via #2667, so in the fork |  |
| #2364 | Convert Negative: film-negative conversion from the Photo menu | LEARN | Convert Negative: not in the fork; upstream re-split it as the still-open #2490 chain || not ported: ~4000 lines and a new film crate that overlaps the fork's film-lab negative filter; the open re-split #2490 belongs to the open-PR review |
| #2348 | Withdrawn: in-app CLIP/YOLO tagging | SKIP | withdrawn by its author; C++ ONNX models |  |
| #2346 | Run batch Reload from Disk on background import workers | SKIP | duplicate: already upstream via #1998, so in the fork |  |
| #2335 | fix(ci): cfg(unix) the staging-test import so Windows clippy passes | SKIP | split or folded upstream into #2334 (reviewed there) |  |
| #2333 | feat: Lightroom-style straighten band, lock-aspect on A and press-started Guided Upright guides | SKIP | split or folded upstream into #2334 (reviewed there) |  |
| #2292 | ui: scrollable profile menu | SKIP | duplicate: already upstream via #2069, so in the fork |  |
| #2262 | LRC-DEV-PROCESS: process V3, over-range colors keep their hue and saturation | LEARN | over-range colours keep hue and saturation: not in the fork; needs its own process-version bump || fork #523 (as process V5) |
| #2229 | engine: refuse malformed mask and spot targets | SKIP | duplicate: already upstream via #2101, so in the fork |  |
| #2228 | engine: refuse fractional command indices | SKIP | duplicate: already upstream via #2100, so in the fork |  |
| #2214 | Web: List every importable raw in the Import Photos picker (#2212) | SKIP | duplicate: #2430 merged the same picker change |  |
| #2201 | Release workflow: pin the shared release workflow to a commit; Dependabot bumps it (#2126) | SKIP | pins upstream's shared release workflow; the fork has its own |  |
| #2200 | Robustness: the 14 findings of #2140 (export names, staging, damaged log, token origin, unpinned downloads, thumbnail decode, hard links, JPEG metadata, latent arithmetic, Windows tests) | ADOPT | partial, 11 of 18 commits (journal copies, thumbnail decode cap, archive without hard links, nn planner, opcodes, fetch token/https, export names, raster overflow, NaN ops, MCP test); the staging-name, APP1-size, subfolder-separator and Windows-test commits conflict with fork code and are left out | fork #515 |
| #2196 | Dependencies: rustls 0.23.46, deny.toml, cargo deny in xtask ci and a weekly Audit workflow (#2127) | ADOPT | partial: the rustls 0.23.46 bump; the fork already runs cargo deny in ci | fork #515 |
| #2193 | denoise-serve: auth deadline over the whole first line; per-address back-off after bad tokens (#2133) | ADOPT | author abandoned it; denoise-serve auth deadline and back-off (security) | fork #515 |
| #2191 | SAM 3 worker: token handshake, concurrent handshakes, survive accept errors (#2134) | ADOPT | author abandoned it; SAM 3 worker token handshake (env names via lcc_brand) | fork #515 |
| #2190 | Develop settings: limits on every list (masks, strokes, samples, spots, red eye, ...) (#2121) | ADOPT | author abandoned it; caps every list in develop settings (never-crash) | fork #515 |
| #2189 | Library folders: never write, cut or read through a planted link (#2122) | ADOPT | partial: lock and store side duplicated by upstream #2552 (now on fork main); the rest (faces index, sidecar backups, thumbnail cache, profiles, read_no_follow) adopted | fork #515 |
| #2186 | Imported profiles: read and delete only the library's own copies in Profiles/ (#2123) | ADOPT | author abandoned it; imported profiles read/deleted only inside Profiles/ | fork #515 |
| #2185 | Edit In: start only the editor set in Settings, never one a client names (#2115) | SKIP | duplicate: the fork fixed #2115 itself (b34d09c20, #479: Edit In starts only editors set in Settings) |  |
| #2183 | engine: stop the export batch's lane queue on every scope exit (#2132) | SKIP | duplicate: already upstream via #2145, so in the fork |  |
| #2182 | Decoder memory budgets: header-only GIF/BMP/JXL sizes, byte budgets, tile and lossy-JPEG limits (#2116, #2117, #2118, #2125) | LEARN | decoder byte budgets (GIF/BMP/JXL headers, tile budget, lossy JPEG): conflicts with fork codecs/raw throughout; re-implement as a follow-up || fork #527 (codecs side; raw batching/lossy JPEG on the upstream-learn2 branch) |
| #2171 | mcp: file-writing helpers report destructiveHint: true (#2139) | SKIP | duplicate: already upstream via #2152, so in the fork |  |
| #2167 | M12: SAM 3 on the GPU outside macOS (wgpu): a photo's analysis ~33 → ~0.6 s, a description ~5 → ~0.5–0.7 s | SKIP | split or folded upstream into #2533 (reviewed there) |  |
| #2163 | LR-PREF-INTERFACE: grid badges on Stars on hover keep flags and labels on every tile, stars only on hover or selection | SKIP | split or folded upstream into #2164 (reviewed there) |  |
| #2157 | Never crash: Cap aggregate TIFF IFD entry allocations (#2136) | SKIP | duplicate: already upstream via #2178, so in the fork |  |
| #2089 | Use the browser's languages as the default UI language on wasm | SKIP | duplicate: #2181 reads the browser languages the same way |  |
| #2038 | Add Windows-only VRR flicker workaround setting | SKIP | duplicate: already upstream, so in the fork |  |
| #2036 | Reject out-of-range hour, minute and second in normalize_iso | SKIP | duplicate: already upstream via #2044, so in the fork |  |
| #2032 | #1999: starred presets in a Favorites section on top of the Presets panel | SKIP | duplicate: already upstream via #2014, so in the fork |  |
| #2031 | #1950: menu paths use → instead of ▸, which no UI font has | SKIP | duplicate: already upstream via #2016, so in the fork |  |
| #2017 | LR-PRE-PANEL: favourite presets listed first in a Favorites group, as favourite profiles are | SKIP | duplicate: already upstream via #2032, so in the fork |  |
| #2008 | Activity & notifications on the top bar's bell; import warnings in a report instead of a screen-filling toast | SKIP | duplicate: already upstream via #2257, so in the fork |  |
| #2005 | Gap 9: run cargo xtask ci on pull requests (Linux x64), tests on macOS and Windows | SKIP | duplicate: already upstream via #1148, so in the fork |  |
| #2001 | Local: show a folder's files as they load; read only raw headers when browsing | SKIP | split or folded upstream into #2477 (reviewed there) |  |
| #1994 | DNG: lossy JPEG XL tiles that declare an sRGB, BT.709 or gamma curve decode at full size | SKIP | duplicate: already upstream via #2003, so in the fork |  |
| #1986 | Fix clippy 1.97 drop_non_drop in tests_export | SKIP | duplicate: already upstream, so in the fork |  |
| #1982 | Android M1: <app>-mobile engine handle and JNI bridge | SKIP | the fork has its own Android port (own design) |  |
| #1954 | LC-SAFETY: Export Keywords refuses an original or its sidecar (#1944) | SKIP | duplicate: #2015 fixed #1944 the same way |  |
| #1934 | LRC-LIB-LIBFOLDERS: fix Windows filesystem test failures | SKIP | duplicate: #1975 and #1967 fix the same Windows tests |  |
| #1929 | Update app descriptions and add new apps | SKIP | craft-launcher app list, not ours |  |
| #1887 | LR-MERGE-HDR: sharp handheld HDR merges (Lanczos-3 warps, full-size and local alignment) | SKIP | split or folded upstream into #2481 (reviewed there) |  |
| #1881 | JPEG XL: check dimensions before full input decode (#1517) | SKIP | duplicate: already upstream via #16, so in the fork |  |
| #1879 | HEIF: bound aggregate decoded grid pixels (#1518) | SKIP | duplicate: already upstream via #17, so in the fork |  |
| #1860 | Brush mask: one stroke densifies into at most a million dabs (#1520) | SKIP | duplicate: already upstream via #1892, so in the fork |  |
| #1853 | Web: optional craft-runtime worker smoke page (no wasm) | SKIP | CollabCraft smoke page, not a merge path |  |
| #1827 | LR-BEHAV-EMPTY-STATES: the samples banner can't be missed (stacked on #1826) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1826 | LR-BEHAV-EMPTY-STATES: the samples say what they bring (stacked on #1825) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1825 | LR-BEHAV-EMPTY-STATES: the first edit on a sample gets a payoff (stacked on #1824) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1824 | LR-BEHAV-EMPTY-STATES: the welcome stands out from the window around it (stacked on #1811) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1819 | fix(lightroom): read large catalogs through bounded file-backed pages | LEARN | lrcat WAL streaming and per-page cancellation (not in #842): worth adding to the fork's catalog import || already in the fork (08f282474) |
| #1811 | LR-BEHAV-EMPTY-STATES: the welcome and the samples banner use the button component (stacked on #1803 and #1810) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1803 | LR-BEHAV-EMPTY-STATES: every way to the samples leads to the editor, which invites a first edit (stacked on #1799) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1799 | LR-BEHAV-EMPTY-STATES: the welcome reads for everyone (stacked on #1789) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1789 | LR-BEHAV-EMPTY-STATES: the welcome has the window, with sample photos to open (stacked on #1781) | SKIP | split or folded upstream into #1781 (reviewed there) |  |
| #1783 | test(gpu): isolate init marker lifecycle tests from concurrent device work | SKIP | duplicate: already upstream via #1758, so in the fork |  |
| #1770 | LR-IMP-CAMERA-COVERAGE: Nikon D600, D610 and D5000 NEFs drop the columns past the image (#1627) | SKIP | duplicate: already upstream via #1053, so in the fork |  |
| #1749 | i18n: add Italian locale | SKIP | duplicate: already upstream via #1198, so in the fork |  |
| #1748 | LRC-LIB-CATALOG-IMPORT: Leave out Lightroom images without a file record instead of failing (#1588) | SKIP | duplicate: already upstream via #2230, so in the fork |  |
| #1720 | Linux: build an Arch package (.pkg.tar.zst) | SKIP | Arch package built by the shared workflow |  |
| #1695 | LR-MAP-MODULE: Map view of the filmstrip's photos, stacked by place | SKIP | duplicate: the Map module came via #797; the fork has Map |  |
| #1680 | LR-VIEW-MOBILE-LAYOUT: an opt-in mobile arrangement (photo-first, panels in a bottom accordion) | SKIP | closed at the author's request; the fork has its own mobile work |  |
| #1678 | Library: drag photos from the grid onto a Folders row to move them there | SKIP | duplicate: already upstream via #1834, so in the fork |  |
| #1672 | Process V3: new sharpening, Masking and colour noise reduction (CPU and GPU) | SKIP | duplicate: Radius/Detail sharpening reached the fork as process V4 (upstream #757/#762) |  |
| #1668 | Never-crash fixes: #1511, #1515, #1516, #1519 and the AllMasks overlay union (#1658) | SKIP | duplicate: fixes landed via #1535 and others; the fork's box_radii already guards NaN and clamps sigma |  |
| #1647 | Configure photo wheel zoom and panning with editing tools | SKIP | duplicate: already upstream, so in the fork |  |
| #1623 | i18n: add Polish interface language | SKIP | duplicate: already upstream via #1100, so in the fork |  |
| #1600 | fix(tiff): don't compute next-IFD offset from a truncated BigTIFF entry count (#1516) | SKIP | duplicate: already upstream via #1544, so in the fork |  |
| #1599 | Fix all-masks overlay CI regression and cover every view | SKIP | duplicate: already upstream via #1599, so in the fork |  |
| #1582 | Packaging: show the installer's completion page (fixes #790) | SKIP | duplicate: already upstream via #1569, so in the fork |  |
| #1494 | Add coverage tests for the AllMasks union fix (#1457) | SKIP | test-only follow-up; the AllMasks fix landed via #1456 |  |
| #1398 | Lightroom import: count the originals while they are probed, not only after all of them | SKIP | duplicate: already upstream via #1024, so in the fork |  |
| #1391 | Fix on_exit arity (GUI does not compile) and embed craft-fonts by default | SKIP | on_exit fix already on main; a CRAFT_FONTS_DIR default would break builds without craft-fonts |  |
| #1379 | Type: emphasis shows, and the smaller repairs of the critique (stacked on #1378) | SKIP | split or folded upstream into #1374 (reviewed there) |  |
| #1378 | Type: longer words keep to their place in Edit, the choices and the filter bar (stacked on #1377) | SKIP | split or folded upstream into #1374 (reviewed there) |  |
| #1377 | Type: an exposure reads one way, and values stand in columns (stacked on #1376) | SKIP | split or folded upstream into #1374 (reviewed there) |  |
| #1376 | Type: marks and words over a picture are in one light ink on a shade (stacked on #1375) | SKIP | split or folded upstream into #1374 (reviewed there) |  |
| #1375 | Type: what the interface writes can be read in every theme (stacked on #1374) | SKIP | split or folded upstream into #1374 (reviewed there) |  |
| #1359 | Display profile: zoomed-in windows are drawn again with a monitor profile in use | SKIP | duplicate: the fix landed via #1036; only a test remains |  |
| #1356 | Fix Intel Mac crash: probe Metal shader language version support | SKIP | vendors a 59k-line patched wgpu-hal; take the fix (gfx-rs/wgpu#10592) through a wgpu bump instead |  |
| #1333 | Settings: a short way by keyboard, and one look for the focus (stacked on #1330) | SKIP | split or folded upstream into #1287 (reviewed there) |  |
| #1330 | Settings: the filter finds what is typed, best first, and shows why (stacked on #1326) | SKIP | split or folded upstream into #1287 (reviewed there) |  |
| #1329 | Settings: where a library's backups stand comes first, with a sign (stacked on #1303) | SKIP | split or folded upstream into #1303 (reviewed there) |  |
| #1326 | Settings: readable in every theme, and in every language it has (stacked on #1321) | SKIP | split or folded upstream into #1287 (reviewed there) |  |
| #1321 | Settings: a filter that finds a section on whatever tab it is (stacked on #1309) | SKIP | split or folded upstream into #1287 (reviewed there) |  |
| #1317 | fix(macos): cargo xtask ci passes on macOS | SKIP | duplicate: already upstream via #997, so in the fork |  |
| #1309 | Settings: tabs grouped by task, with Library and Files (stacked on #1299) | SKIP | split or folded upstream into #1287 (reviewed there) |  |
| #1299 | Settings: ask before what cannot be taken back, and Esc in a text box gives the text back (stacked on #1289) | SKIP | split or folded upstream into #1287 (reviewed there) |  |
| #1289 | Settings: tabs look like tabs, and what is chosen or has the focus shows (stacked on #1287) | SKIP | split or folded upstream into #1287 (reviewed there) |  |
| #1266 | Delete asks first by default, from the context menu too | SKIP | duplicate: already upstream via #1242, so in the fork |  |
| #1256 | Read Exif and XMP embedded in HEIF and AVIF. | SKIP | duplicate: already upstream via #1208, so in the fork |  |
| #1252 | Ask before deleting a photo by default. | SKIP | duplicate: already upstream via #1242, so in the fork |  |
| #1199 | fix: Local's folder rows sat 12 pt right of the other sidebar trees | SKIP | duplicate: already upstream via #1102, so in the fork |  |
| #1197 | Fixed some bug that prevented the GUI from compiling properly | SKIP | duplicate: already upstream, so in the fork |  |
| #1175 | Issue #1080: `run --import <folder>` takes HEIC, HEIF and NRW like library.import | SKIP | duplicate: already upstream via #1119, so in the fork |  |
| #1143 | Filmstrip and grid follow the active photo to its place in another source | SKIP | duplicate: already upstream via #1253, so in the fork |  |
| #1121 | LRC-LIB-UPDATEFOLDER: Update Folder Location for agents (folder.updateLocation, #1116) | SKIP | duplicate: already upstream via #1122, so in the fork |  |
| #1089 | fix(history): restore steps without appending recursive history | SKIP | duplicate: already upstream via #1246, so in the fork |  |
| #1088 | perf(catalog): cache folded filename sort keys | SKIP | main sorts by natural order now; this cache key would change the order |  |
| #1085 | Fix/rotate crops image issue #962 | SKIP | duplicate: #966 landed the rotate fix |  |
| #1084 | macOS CI, nightly (gap 9); the corpus test skips hidden files | SKIP | duplicate: already upstream via #989, so in the fork |  |
| #1083 | Fix the macOS build and macOS test failures; run cargo xtask ci on macOS | SKIP | duplicate: already upstream via #989, so in the fork |  |
| #1068 | Keep parser guards compatible with Rust 1.95 and macOS CI | SKIP | duplicate: already upstream via #997, so in the fork |  |
| #1061 | fix: export 'Same folder as original' option (issue #956) | SKIP | duplicate: already upstream, so in the fork |  |
| #1055 | fix: Before側クリックのズーム位置を正しく計算する | SKIP | duplicate: already upstream via #1742, so in the fork |  |
| #1044 | fix: 選択済みのPick旗を塗りつぶして表示する | SKIP | duplicate: already upstream via #1863, so in the fork |  |
| #1041 | fix(export): render exports at exactly their resolved size (LR-EXP-DIM) | SKIP | duplicate: already upstream via #1338, so in the fork |  |
| #1039 | fix: 切り抜きの初期比率をOriginalでロックする | SKIP | duplicate: already upstream via #1864, so in the fork |  |
| #1012 | Local adjustment presets, user DCP camera profiles, film grain, GPU rendering, preset performance, Windows fixes | SKIP | split or folded upstream into #1052 (reviewed there) |  |
| #994 | Fix macOS build: menu-bar Quit no longer calls eframe's on_exit | SKIP | duplicate: already upstream, so in the fork |  |
| #983 | Edit in PhotoCraft: find the installed PhotoCraft and open the edit copy in it (#918) | SKIP | duplicate: main got its own Edit in PhotoCraft (44b12efe) |  |
| #979 | Improve RAW normalization, native lens corrections and mask integrity | SKIP | split or folded upstream into #1068 (reviewed there) |  |
| #969 | Edit In: a repeat copy works, the right-clicked photo is copied, failures are shown, PhotoCraft found in non-ASCII folders (#918) | SKIP | superseded by #920 (Edit In for every selected photo) |  |
| #968 | Local: a folder shown under Home and as a saved root expands in each tree (#893) | SKIP | duplicate: already upstream, so in the fork |  |
| #949 | Fix/exif histogram overlap | SKIP | duplicate: already upstream via #913, so in the fork |  |
| #941 | ci: add Linux x64 CI workflow (partially closes gap #9) | SKIP | upstream CI workflow; the fork has its own self-hosted CI |  |
| #940 | UI: dialogs fit the screen: fixed header and footer, scrolling body | SKIP | duplicate: already upstream via #786, so in the fork |  |
| #938 | LR-PROF-CAMERACOLOR: a tone curve from black-and-white camera JPEGs (#577) | SKIP | duplicate: already upstream via #981, so in the fork |  |
| #933 | Use original letter monogram app icon | SKIP | icon change; the fork derives icons from brand.toml |  |
| #915 | LR-IMP-FORMATS: decode Canon EOS R5 Mark II C-RAW (CRX QP-map first literal and escape) | SKIP | duplicate: already upstream via #981, so in the fork |  |
| #909 | Feature/ai masking | SKIP | pulls in ONNX Runtime (C++), so not pure Rust; the fork has candle SAM 3 masks |  |
| #908 | M7.1: capture sharpening radius and full-size export defaults | SKIP | raises sharpening without a process-version bump; fork sharpening is process V4 (Radius/Detail) |  |
| #897 | Metadata: All Metadata shows the raw image's compression, Nikon's in words (#830) | SKIP | duplicate: already upstream via #831, so in the fork |  |
| #889 | Dialogs: information-only dialogs close with one button (#782) | SKIP | duplicate: already upstream via #782, so in the fork |  |
| #888 | xtask web: the site leaves <target>/web to cargo; the release zip ships only the site (#793) | SKIP | duplicate: already upstream via #848, so in the fork |  |
| #886 | feat(theme): add Light and System interface appearance | SKIP | duplicate: already upstream via #950, so in the fork |  |
| #882 | feat(ui): standardize window decorations and title bar with VectorCraft (#881) | SKIP | empty placeholder PR |  |
| #876 | fix(ui): cap dialog height to the viewport so tall ones (Export) scroll (close #781) | SKIP | duplicate of the dialog-height fix landed via #786 |  |
| #872 | docs: point optional font setup to public integration guide (#826) | SKIP | superseded by #890 |  |
| #852 | System Info, All Metadata and What's New end with one Close button, like About (#782) | SKIP | duplicate: already upstream via #782, so in the fork |  |
| #851 | control protocol: document the Wayland visibility stall (#718) | SKIP | duplicate: already upstream via #1070, so in the fork |  |
| #849 | raw: probe the DSC-F828 SRF and DSLR-A100 ARW as Sony raws (#702) | SKIP | duplicate: already upstream via #702, so in the fork |  |
| #846 | Catalog: segments of about 2 MB and at most 4096 photos (stacked on #814) | SKIP | split or folded upstream into #814 (reviewed there) |  |
| #838 | Test/upstream sync | SKIP | bulk sync of unrelated changes |  |
| #837 | VERA-1: <app>-vera crate — keyword index, PDQ/pHash digests, RRF… | SKIP | standalone uncalled search crate, nothing wired to it |  |
| #789 | Sidebar sections: reorder, show/hide, pinned headers, shared section frame | SKIP | duplicate: already upstream via #981, so in the fork |  |
| #786 | Dialogs fit the window: options scroll, the title and Cancel / OK stay visible (#781) | SKIP | duplicate: already upstream via #786, so in the fork |  |
| #780 | LR-EXP-TYPE: JPEG XL export (#247) | SKIP | duplicate: already upstream via #998, so in the fork |  |
| #766 | Brush mask: the live stroke places dabs like the committed one, no gaps on fast strokes (#517) | SKIP | duplicate: already upstream via #766, so in the fork |  |
| #761 | docs/parity.md: Top gaps lists items 1 and 2 once, with the current facts (#756) | SKIP | duplicate: already upstream via #788, so in the fork |  |
| #752 | Kelvin white balance for raws whose camera has colour matrices, with a look-preserving migration (#730) | SKIP | duplicate: already upstream via #981, so in the fork |  |
| #751 | Crop angle set through develop.set / MCP / --set now keeps the crop inside the photo, as Straighten does (#742) | SKIP | duplicate: already upstream via #751, so in the fork |  |
| #739 | Crop tool: no dark wedges inside the crop box when the crop is rotated (#731) | SKIP | duplicate: already upstream via #739, so in the fork |  |
| #733 | M11.4: reject illuminated Canon CR2 border samples when estimating black levels | SKIP | duplicate: already upstream, so in the fork |  |
| #727 | NEF: probe compressed files from their first 128 rows, not the whole Huffman stream (#708) | SKIP | duplicate: already upstream via #727, so in the fork |  |
| #722 | Lightroom Classic + Immich | SKIP | the fork itself |  |
| #713 | LRC-LIB-CATALOG-LOCK: release the library lock explicitly on drop (#680) | SKIP | duplicate: already upstream via #710, so in the fork |  |
| #711 | Sony ILCE-7CM2: #697 reproduced on 0.5.0, fixed on main since 3100cc1a; corpus coverage for its codings | SKIP | duplicate: already upstream via #711, so in the fork |  |
| #690 | Albums hold library photos: skip Local photos, refuse a Local-only selection, say to Add to My Photos first (#684) | SKIP | duplicate: already upstream via #690, so in the fork |  |
| #668 | MCP: cancel the e2e export at its first progress notification | SKIP | duplicate: already upstream via #674, so in the fork |  |
| #660 | Synchronize Folder tests: expected paths spelled as the OS does (Windows) | SKIP | duplicate: already upstream via #873, so in the fork |  |
| #624 | ci: Add RISC-V Linux build with deb, rpm, and tar.gz packaging | SKIP | RISC-V job landed via #844 |  |
| #602 | LR-EDIT-LIGHT-HIGHLIGHTS: partly clipped highlights take the colour of bright neighbours; previews clip before averaging (#548) | SKIP | duplicate: already upstream via #607, so in the fork |  |
| #595 | Publish Flatpak repository through GitHub Pages | SKIP | Flatpak via Pages, superseded |  |
| #594 | Add semantic-release automation on main | SKIP | semantic-release, not wanted |  |
| #573 | LR-IMP-FORMATS: decode packed 12-bit Sony ARW (DSLR-A900) from its exact 1.5 bytes per pixel strip | SKIP | duplicate: already upstream via #692, so in the fork |  |
| #551 | LR-CROP-ROTATE-DRAG: draw a rotate pointer outside the crop box (issue #534) | SKIP | duplicate: already upstream via #534, so in the fork |  |
| #544 | Add OpenWiki automation and Pages publishing | SKIP | Copilot/OpenWiki automation, not wanted |  |
| #539 | Add offline GPS map projection primitives | SKIP | duplicate: already upstream via #797, so in the fork |  |
| #537 | LRC-LIB-CATALOG-IMPORT: read Lightroom's Custom white balance by its as-shot shift on relative-WB raws (#510) | SKIP | duplicate: the fork reads relative Custom WB (crs.rs, lightroom_catalog.rs tests) |  |
| #488 | LRC-LIB-CATALOG-IMPORT: import crops from catalog develop settings (#483) | SKIP | duplicate: already upstream via #489, so in the fork |  |
| #481 | Advance selection to next photo after deleting (#441) | LEARN | neighbour selection after photo.deletePermanently, the part #451 didn't cover || already in the fork (c0ff290e7) |
| #467 | Issue #441: keep the grid's scroll position when deleting photos | SKIP | duplicate: already upstream via #441, so in the fork |  |
| #466 | HDR editing, Visualize HDR range, headroom limit, HDR export and HDR display | SKIP | the fork already ships HDR editing and display (0.8.2) |  |
| #456 | Fix/issue 425 missing simplified chinese font | SKIP | CJK fonts come from craft-fonts in the fork |  |
| #428 | Compare web window jobs against the CPU reference | SKIP | duplicate: already upstream via #426, so in the fork |  |
| #418 | Fix translated menu symbols rejected by font-enabled CI | SKIP | duplicate: already upstream via #411, so in the fork |  |
