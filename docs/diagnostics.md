# Diagnostics recorder

`crates/diag` (`lcc-diag`, layer L0) answers one question: *what was this process doing when it hung, was killed,
ran out of memory or panicked?* It is off unless `<PREFIX>_DIAG` is set, writes only to a local folder, and sends
nothing anywhere (issue #1363).

## Turning it on

| Variable | Meaning |
|---|---|
| `<PREFIX>_DIAG` | Levels to record. A bare level sets every subsystem, `name=level` overrides one; order doesn't matter and the last entry for a subsystem wins. Words are not case-sensitive. `1`, `on`, `true` and `yes` mean `normal`; `0`, `off`, `false` and `no` mean off; unset, empty, or a list that records nothing (`off`, `off,probe=0`) leaves the recorder off. |
| `<PREFIX>_DIAG_DIR` | Where the files go. Default: `diag/` in the settings folder (the one `<PREFIX>_CONFIG_DIR` and portable mode choose). Set but empty: no folder. |

Levels, least verbose first: `critical` (a step that runs once per user action and matters after a kill, and panics:
written to disk at once), `normal` (stages and per-batch steps), `detail` (work that runs after every command),
`trace` (per-item work: one file, one photo). Examples: `normal`, `critical,commit=trace`,
`normal,catalog=detail,probe=off`. Entries that aren't understood are ignored and counted in one `config_rejected`
event.

Subsystems: `app` (the recorder itself: start, panics, configuration), `import` (the commit of each import batch),
`probe` (reading files before an import, a reload, a recheck or reading originals), `commit` (a Lightroom catalog
import's commit and its stages), `catalog` (writing the library after a command).

A run that keeps no settings (`--memory`, `--demo`, `<PREFIX>_NO_PREFS`) records only when `<PREFIX>_DIAG_DIR` names a
folder. `--version` and `--help` record nothing. The app and the CLI log where they record, or why they don't
(no folder, a folder that can't be created).

## What it writes

Per process, in the folder, named `<kind>-<pid>-<start time in ms>.json` (so a later process with the same pid never
overwrites an earlier one's files):

- `inflight-…`: the steps open right now, with how far each has got, and whether the process ended cleanly. A process
  that hangs, is killed or runs out of memory leaves this file behind with no clean end, saying what was running.
- `ring-…`: the newest 512 finished steps and events. When it is full, a newcomer pushes out an entry at least as
  verbose as itself (the most verbose level first, within it a routine success, else the oldest); when everything in
  it is less verbose, the newcomer is dropped. Every drop is counted.
- `stats-…`: per-name count, failures, total, minimum and maximum time of per-item work (one probed file, one applied
  photo), and what the recorder itself cost.

When a process starts, older processes' files are pruned: the newest 12 that ended cleanly and the newest 24 that
did not are kept, so a run of clean starts never removes the record of a hang. Only files named as above are ever
removed; anything else in the folder is left alone.

Fields hold ids, counts and short names, never file names, paths or tag values. A panic is recorded with its source
file's name (not its path), line and column. Every subsystem, outcome, level and kind is a closed enum: nothing
parses a message string.

`cargo run -p lcc-diag --example report -- <dir>` prints a report of every process that left files there: ended
cleanly, running, or ended without a clean exit; what was in flight; the last entries; where the time went; and the
recorder's own cost. Files it can't read (damaged, too large, written by another version) are named, not skipped.
Whether a process is still running is asked with `ps`, so outside Unix the report says it doesn't know.

## How it writes

When off, every call is one atomic load. When on, a call below its subsystem's threshold is still one load. A
`critical` record is written before the call returns; everything else by a background thread at most every 250 ms, so
working threads never wait for the disk. Writes go through one path, never older than what is on disk: a slow
background write can't land over a newer critical one. Each file is written to a freshly created temp file and
renamed into place. A clean end (`lcc_diag::finish`, on every orderly way out of the app and the CLI, including the
macOS ones that end with `process::exit`) marks the in-flight file and writes everything; a kill or a crash leaves the
last state written.

A panic is recorded after the existing panic hooks have run (stderr, the panic log, the log file). If another thread
holds the recorder's lock at that moment, the panic is counted as lost instead of waited for.

`cargo run --release -p lcc-diag --example bench` measures the hot path (on an Apple M4 Max: a per-item span about
50 ns from 1 or 8 threads, a step about 0.2 µs). With an earlier build of this recorder (more call sites than this one) on a
one-million-photo Lightroom import of about 505 s, 1,000,050 recorded calls cost 51 ms on the working threads and
0.73 s in the writer thread.

## Call sites

- `probe_all` (`crates/engine/src/import.rs`): `normal`, once per call (a batch, usually), with that call's file count
  and how many are probed; `cancelled` when files were left unprobed. Each file as a `trace` item (`probe_file`).
- `import_commit` (`import::commit_prepared`): `normal`, once per batch.
- `lightroom_commit` (`lightroom_job::commit_prepared`): `critical`, once per catalog import, with the photo count;
  its stages `apply_photos` (with how many photos are finished), `apply_collections` and `index_json` at `normal`, and
  each photo as a `trace` item (`apply_photo`).
- `persist` and `flush_catalog` (`crates/engine/src/library.rs`): `detail`, since they run after every command.
- Start (`lcc_engine::logging::start_diagnostics`) and the clean end (`lcc_diag::finish`): the desktop app
  (`apps/app/src/main.rs`) and the CLI (`apps/cli/src/lib.rs`).

A step that returns a `Result` goes through `lcc_diag::traced`, which ends it with the real outcome on every path:
`ok`, `failed`, or `cancelled` for `EngineError::Cancelled` (`lcc_engine`'s `diag_outcome`). A stage inside a longer
function is a `Span` ended with `finish`; if an error leaves the function early it ends `abandoned`, and the step
around it records the error. Per-item work in a loop uses `lcc_diag::item_timer`, which ends `ok` when dropped. Pick
the level by how often the step runs: `critical` only for something that happens once per user action.
