# Release versions

Releases are numbered by the week they ship, not by `0.x` semantic versions. The last numbered release was 0.9.6.

## Scheme

`vYY.WW.B`: the ISO week-numbering year (two digits), the ISO week (1–53), and the release's number within that
week, from 1. The counter starts again at 1 each week. The tag and the version are the same string (the tag adds
`v`), with no leading zeros, so it is a valid semantic version everywhere.

| Release | Tag | Version |
|---|---|---|
| first in week 43 of 2026 | `v26.43.1` | `26.43.1` |
| fourth that week (22 Oct 2026) | `v26.43.4` | `26.43.4` |
| first in week 1 of 2027 | `v27.1.1` | `27.1.1` |

**Use the ISO week-year, not the calendar year.** Around New Year the two differ: 1 Jan 2027 is in week 53 of 2026
(`v26.53.n`), and 30 Dec 2025 is in week 1 of 2026 (`v26.1.n`). Taking the calendar year there would put a release
behind the one before it. `date +%G.%V` prints the year and week (`2026.43`).

A prerelease is `-rcN` on the version it leads up to (`v26.43.4-rc1`). The changelog file is
`docs/changelog/26.43.4.md`. Compared as numbers (`git tag --sort=v:refname`), every release is newer than the one
before. Compared as plain text it isn't (`26.9` vs `26.10`).

## Package formats

- **Windows MSI** `ProductVersion` (`major.minor.build`, major and minor ≤ 255, build ≤ 65535): fits.
- **Android:** `versionName` is the version. `versionCode` keeps its formula `YY·10⁶ + WW·10⁴ + B·100 + 99`,
  and `-rcN` uses `N` in place of 99 (`26.43.4` → 26 430 499). This allows up to 99 releases a week and fits under
  Android's 2 100 000 000 limit until year 2099.
- **macOS** `CFBundleShortVersionString` / `CFBundleVersion` (three integers): fits.
- **Cargo, the in-app updater, `.deb`, `.rpm`, Flatpak / AppStream:** ordinary `X.Y.Z` and compared correctly.

## Release steps

1. `date +%G.%V` gives `YY.WW` (drop the century, and any leading zero from the week).
2. B is one more than the latest `vYY.WW.*` tag, or 1 if there is none.
3. Set `version` in the root `Cargo.toml`, then release as before (`cargo xtask notes --release <version>`).
