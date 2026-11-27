# Diagnostics recorder

`crates/diag` (`lcc-diag`, layer L0) answers one question: *what was this process doing when it hung, was killed,
ran out of memory or panicked?* It is off unless `<PREFIX>_DIAG` is set, writes only to a local folder, and sends
nothing anywhere (issue #1363).

## Turning it on

| Variable | Meaning |
|---|---|
| `<PREFIX>_DIAG` | Levels to record. A bare level sets every subsystem, `name=level` overrides one; order doesn't matter and the last entry for a subsystem wins. `1`, `on` and `true` mean `normal`; `0` or `off` (or unset) means off. |
| `<PREFIX>_DIAG_DIR` | Where the files go. Default: `diag/` in `<settings_dir>`. |

Levels, least verbose first: `critical` (the outermost steps and panics, written to disk at once), `normal` (stages
of a step), `detail` (work that runs after every command), `trace` (per-item work: one file, one photo). Examples:
`normal`, `critical,commit=trace`, `normal,catalog=detail,probe=off`. An entry that isn't understood is ignored and
recorded as a `config_rejected` event.

Subsystems: `app` (the recorder itself: start, panics, configuration), `import` (the import commit), `probe` (reading
each file before an import), `commit` (a Lightroom catalog import's commit and its stages), `catalog` (writing the
library after a command), `control` and `ui` (no call sites yet).

## What it writes

Per process, in the folder (the newest 12 processes are kept):

- `inflight-<pid>.json`: the steps open right now, with how far each has got. A process that hangs, is killed or
  runs out of memory leaves this file behind, saying what was running.
- `ring-<pid>.json`: the newest 512 finished steps and events. When full, the most verbose level goes first, then a
  routine success, then the oldest, so a failure or a critical record outlives a burst of detail.
- `stats-<pid>.json`: per-name count, total, minimum and maximum time of per-item work (one probed file, one applied
  photo), and what the recorder itself cost.

Fields hold ids, counts and short names only, never file names, paths or tag values. Every subsystem, outcome, level
and kind is a closed enum: nothing parses a message string.

`cargo run -p lcc-diag --example report -- <dir>` prints a report of every process that left files there: running or
gone, what was in flight, the last entries, where the time went and the recorder's own cost.

## Cost

When off, every call is one atomic load. When on, a call below its subsystem's threshold is still one load. Records
below `critical` are written by a background thread at most every 250 ms; the working thread never waits for the
disk. `cargo run --release -p lcc-diag --example bench` measures the hot path (on an Apple M4 Max: a per-item span
about 45 ns at 8 threads, a step about 0.2 µs). With an earlier build of this recorder (more call sites than
this one) on a one-million-photo Lightroom import of about 505 s, 1,000,050 recorded calls cost 51 ms on the working
threads and 0.73 s in the writer thread.

## Call sites

- `probe_all` (`crates/engine/src/import.rs`): `critical`, with the number of files and how many are done; each file
  as a `trace` item.
- `import_commit` (`import::commit_prepared`): `critical`.
- `lightroom_commit` (`lightroom_job::commit_prepared`): `critical`, with the number of photos; its stages
  `apply_prepared`, `apply_photos` (with how many are done), `apply_collections` and `index_json` at `normal`, and
  each photo as a `trace` item.
- `persist` and `flush_catalog` (`crates/engine/src/library.rs`): `detail`, since they run after every command.
- Process start and a final flush: the desktop app (`apps/app/src/main.rs`) and the CLI (`apps/cli/src/lib.rs`).

A new call site: a `Span` from `lcc_diag::span` / `span_at`, ended with `finish(outcome)` or `done(&result)`; a span
dropped without either ends `abandoned` (`panicked` while unwinding). Pick the level by how often it runs.
