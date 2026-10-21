# Faster analysis and USB export — design

Date: 2026-10-21. Fork: `michaelmenesescs/rbxport` (fork-only; not intended for upstream PRs).

## Why

The owner keeps ~6,700 tracks on an external SSD ("Overflow") and builds several Pioneer USB sticks at a time, both fresh sticks and updates to sticks that already hold the library. Today a full analysis of the library takes about 70 minutes and a fresh export spends most of its time flushing, not copying. The owner's goal, in their words: "I want tracks to export and analyze much faster." The bottleneck should be the SSD/USB bandwidth, not rbxport's bookkeeping.

## Evidence (measured 2026-10-21)

Rigs added in this fork, all run on a Mac mini M4 (10 cores) against copies:

- `crates/rbl-analysis/examples/throughput.rs` — decode + DSP only, 24 real MP3s (157 min of audio):
  1 worker 1.21 tracks/s; 4 workers 3.99; 8 workers 4.64. Per track: decode 482 ms, DSP 344 ms.
- App, live (MacBook Pro M1 Max, 4 slots): ~97 tracks/min (1.6/s). From the log, 3,844 library writes in 10 min for 961 tracks; 363 s of the 600 s were spent inside writes (252 s just opening the database, 65 ms each), all serialised behind `edit_gate`.
- `crates/rbl-db/examples/write_cost.rs` — the analysis save path against a copy of the real `master.db`, 200 tracks:
  writer per edit (today) 310 ms/track; one writer per track 112 ms; one writer for the batch 5.2 ms. `Writer::open` alone is ~100 ms (SQLCipher key derivation + pragmas); the rekordbox-running check is 7 ms.
- `crates/rbl-export/examples/export_cost.rs` — 6,741 files (200 KB stand-ins) to a folder on internal SSD:
  fresh export 90.8 s, of which publishing 79.7 s, copying 8.5 s, database 1.2 s. Re-export unchanged 0.3 s; re-export with metadata changes 4.1 s (files were page-cached, so the read-back cost below is hidden).

## Goals

1. **Analysis saves without reopening the library per edit.** Target: app throughput reaches the DSP ceiling (~240+ tracks/min at 4 slots on the M1 Max), i.e. ≥2.5× today.
2. **Analysis concurrency scales to the machine.** The "Tracks analysed at once" choice allows up to the number of performance cores (or a sensible max such as 8), defaulting to something better than 3–4.
3. **Fresh export publishing costs a handful of drive flushes, not two per file.** Target: publishing of 6,741 files under 5 s on internal SSD (from 80 s), with a correspondingly large drop on USB sticks.
4. **Updating a stick does not read every unchanged track back off the stick.** Unchanged audio is trusted from the previous export's receipt (size + mtime + recorded hash) by default; the existing full check stays available as "verify all".
5. **Decode is cheaper.** Overlap decode with DSP and/or avoid full-rate work the DSP doesn't need, without changing analysis results.

## Non-goals

- Changing analysis *results* (BPM, grid, key, waveform) — outputs must be byte-identical to today's for the same input, or the change is out.
- New UI beyond widening the existing concurrency choice and any progress text the changes need.
- Windows/Linux tuning (keep them compiling and correct; measure only on macOS).
- Upstreaming. Code style should still match the codebase so merging upstream later stays possible.

## Anti-goals (would make this a failure even if it's faster)

- **Weaker crash safety.** A crash, unplug or power loss mid-export must still leave the stick as either the old or the new generation, recoverable through the existing journal. A crash mid-analysis must still leave the library consistent and the file journal recoverable.
- **Locking rekordbox out.** The app opens the library per edit so rekordbox launched behind it can take the file back. Holding a writer must be bounded (released between batches and on idle), and every batch must still refuse when rekordbox is running.
- **Freezes.** No new main-thread waits; today's AppleScript rename deadlock shows how easily the app hangs.

## Constraints

- Workspace lints (`perf = deny`, no `unwrap/expect/panic`, etc.) and the existing test suites must pass.
- Release profile is `lto = "fat"`; benchmark builds may use thin LTO for speed but final numbers should be quoted from a release build.
- macOS `F_FULLFSYNC` semantics: it flushes the file and then asks the drive to flush its whole write cache. Plain `fsync` alone does not guarantee data reached stable storage on Apple hardware.

## Decided approach

### 1. Batched library writes for analysis (biggest win)
- Add a scoped writer session to `AppState` (e.g. `with_writer(|writer| …)`), which opens one `Writer` and runs several edits on it under one `edit_gate` hold. Use it in `analyse_and_save`/`save_analysis_files` so one track's `import_artwork`, `analysis_data_path_for`, `ensure_detected_key`, `set_analysis` (and first-beat cue) share a connection: 310 → ~112 ms.
- Then keep a writer open *across* tracks while an analysis run is active: a cached writer in `AppState` with an idle timeout (proposal: closed after ~2 s without a write, and closed immediately when the run drains, when rekordbox is detected, and before any restore/backup swap). Target ~5 ms/track.
- The rekordbox-running check runs once per track rather than once per edit (today ~4×/track), so behaviour when rekordbox launches is unchanged.
- The rig's 5 ms omits per-track costs the app also pays: `FileJournal::prepare` (≈7 durable writes, each with full + directory flush, plus its own read-only library open) and `ensure_analysis_unlocked` → `grid::database_locked` (two read-only opens). Route the extra opens through the cached connection / one shared read-only handle.
- `edit_gate` is taken **per track**, not held for the writer's lifetime: the user's own edits (ratings, playlists, grid edits, imports) during a run must stay instant. User edits go through the same cached writer when one is open (`AppState::write`/`write_then` reroute through the cache), so there is never a second read-write connection. *(Claude's call: the anti-goal on freezes decides it.)*
- Keep one database commit per track: the file journal's recovery compares each track's row USN with the one it recorded.
- **The owner accepts losing a few tracks' analysis on a crash** (decided 2026-10-21). So `FileJournal` durability may be batched: journal entries and analysis files for several tracks are flushed together (one full flush per small batch or per ~1 s) instead of several full flushes per track. Required: after a crash the library is consistent, recovery finishes or rolls back each journaled track, and at worst the last batch's tracks show as unanalysed and are re-analysed. Changing `file_journal.rs` and its recovery tests is in scope.

### 2. Wider analysis concurrency
- `ANALYSIS_SLOTS` in `src/lib/queue.ts` and the backend's blocking pool must allow more than today. Default to the performance-core count (4 on M1 Max → but measure 6–8 once writes are no longer the bottleneck).

### 3. Batched durability in export publication (`rbl-core::durable::Publication`)
- Replace per-file `F_FULLFSYNC` + per-rename directory `F_FULLFSYNC` with barriers: plain `fsync` (or none) per staged file, then **one** full flush before the journal rename; renames; then one directory flush per touched directory and one final full flush. The journal (`publication.json`, `.previous`, replay in `finish`) is unchanged, so recovery semantics stay the same.
- Prove with the existing durability/recovery tests plus a new test that kills between barriers (fault-injection hooks already exist via `flush_with`).

### 4. Re-export read-back: verify first, fix only if real
- Correction to the earlier claim: `export_cancellable_inner` already trusts the stick copy when its size and mtime match the manifest's `copy_stamp` (`copy_unchanged`), and only hashes it otherwise; `verify_all` forces the full check. The read-back only happens when the stamp doesn't match: manifests predating `copy_stamp`, or FAT/exFAT mtimes shifting (local time, 2 s resolution, DST).
- Builder: re-export to a real FAT32/exFAT stick twice (and once across a simulated timezone change) and log how many tracks take the hash path. Only if that number is material, loosen `same_copy` for FAT (e.g. tolerate 2 s and whole-hour offsets). If it isn't, drop this item. *(Claude's call, from the code.)*

### 5. Decode cost
- Pipeline decode and DSP (decode thread feeding chunks), and investigate whether the tempo/key stages can run on a decimated signal while the waveform keeps full rate. Must keep results identical; if not achievable, drop this item.

## Left to the builder

- Exact API names and module placement; whether the cached writer lives in `AppState` or a new type.
- The idle timeout value (start ~2 s; tune by measurement).
- Ordering of work, but item 1 first (largest, lowest risk), then 3, 4, 2, 5.
- Whether to also parallelise per-track export copying (measure on a real USB stick first; likely bandwidth-bound).

## How we'll know

Re-run the three rigs and the live app on the same inputs before/after each item, quote release-build numbers, and record them in this file. Test on a real USB stick (FAT32/exFAT) for items 3–4 before calling them done.
