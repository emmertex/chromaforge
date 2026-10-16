# lcc-updater

The app's auto updater (L0, native only): it finds a newer release, downloads the file for this
machine, and stages installing it. There is no update service behind it — the release feed is a file
the release pipeline publishes, plus GitHub's own Releases API as a fallback.

```rust
// what is newer than this build?
let opts = lcc_updater::Options::default();          // brand repository, this version, this platform
if let Some(update) = lcc_updater::check(&opts, None, None)? {
    println!("{} → {}", update.current, update.version);   // 0.7.0 → 0.8.0
    println!("{} ({})", update.asset.name, update.action); // the file, and what installing it does
    // fetch it (resumable, size- and SHA-256-checked) into the cache folder
    let got = lcc_updater::download(&update, &lcc_updater::client()?, dir, None, None)?;
    // say what would happen, then do it (installs over the running installation)
    let plan = lcc_updater::plan(update.asset.kind, &got.path, &update.version.to_text());
    let outcome = lcc_updater::install(update.asset.kind, &got, &update.version.to_text())?;
    if outcome.exit { /* quit: the installer or the replacement script takes over */ }
}
```

- **The feed.** `latest.json` on the newest release (`…/releases/latest/download/latest.json`),
  else `GET /repos/{owner}/{repo}/releases` (no token needed for a public repository; a 403 is the
  rate limit, reported as such), else a URL a Rust caller names (`Options::manifest_url`: a mirror
  or a test; no command or setting reaches it). Both shapes parse into `Release`; the newest
  **version** wins, not the feed's order.
- **Where files may come from.** Only `https://github.com/<owner>/<repo>/releases/download/…` of the
  brand's `REPOSITORY` (`release_download_prefix`); a file the feed lists anywhere else is passed
  over, and a feed, file or checksum list on plain `http://` is refused. Tests trust one local
  origin through `Options::extra_origin`.
- **Versions** are SemVer 2.0.0 (`v1.2.3`, `1.2.3-rc.1`); a tag that is not a version (`nightly`) is
  not a release of ours. A prerelease is only offered when asked for, and a release outranks its own
  prereleases.
- **The right file** is picked from the artifact names the pipeline already uses
  (`<binary>-<version>-<os>-<arch>…`): the MSI on Windows, the DMG on macOS, the AppImage on Linux
  x86_64/aarch64, and a tarball (or the portable zip) as a manual install anywhere else. An
  installer always wins over an archive; the `SHA256SUMS.txt` asset is read for the hash, never
  installed.
- **Downloads** resume from a `.part` file, are capped, and must match the size the feed states and
  the SHA-256 the release publishes (the size alone when it publishes none) before they are renamed
  into place. A file that fails its check is deleted. A file already there is re-hashed, not trusted
  by name; a hostile asset name (`../../…`) is reduced to its last path segment.
- **Installing** is `install::plan` (what will happen, in words, before anything does) and
  `install::install`: `msiexec` runs the Windows installer, a macOS script mounts the disk image and
  copies the app over the running one from a terminal window, a Linux script renames the running
  AppImage aside and starts the new one, and anything else is revealed in the file manager for a
  manual unpack. Every path ends with the app running again or with a sentence saying what to do.
- **Never a panic.** No network, a rate-limited API, a release without a file for this machine, a
  truncated download, a full disk, an installer that refuses: all are `UpdateError` values a dialog
  can show, and all leave the current installation untouched.

## Sources

Own design, from the documented [GitHub Releases API](https://docs.github.com/rest/releases) and
the layout of release assets; version precedence follows [Semantic Versioning
2.0.0](https://semver.org/spec/v2.0.0.html). The shape of the flow (check → download → install and
restart, with a manifest beside the release) follows Tauri's updater as a design reference, not its
code.

Tests use local HTTP servers for the feed, the checksum list and the file: they make no internet
requests. The end-to-end path (feed → download → staged file) is covered in `lcc-engine`.
