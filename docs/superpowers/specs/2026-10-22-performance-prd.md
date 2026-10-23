# rbxport fork: fast USB export and analysis — PRD

Date: 2026-10-22. Owner: Michael (fork `michaelmenesescs/rbxport`, branch `perf/analysis-export`).
Supersedes the scope of `2026-10-21-performance-design.md` (keep it for the benchmark evidence and the anti-goals, which still apply).

## Problem

The owner DJs off Pioneer XDJ-700 USB sticks built from a ~6,700-track, ~98 GiB library on an external SSD ("Overflow"). Building a gig stick with rbxport 1.9.0 took over a day of wall time, against roughly an hour for the bytes involved:

| What happened (2026-10-21/22) | Measured |
|---|---|
| Kingston DataTraveler, FAT32, full House folder (3,937 tracks, 61 GB) | copy ~5 h at ~3 MB/s average, then **publishing ran 8 h at ~34 files/min** and was still 5,900 files short when abandoned |
| Lexar 256 GB, FAT32, full library (6,728 tracks) | copy ~80 min (limited by Overflow on a USB 2.0 link, ~30 MB/s); publish ~600 files/min |
| Lexar drops off USB mid-publish (EIO) | stick flagged "needs recovery", hidden from AppleScript; one GUI Sync replayed the journal (good) |
| Follow-up sync with `usbExport.verifyAllFiles=true` | "Verifying" ran >70 min reading the stick **and** every source (333 GiB read) with nothing to copy |
| AppleScript `export playlist … to device` on a long export | fails after exactly 6 h with "rbxport's window did not answer" while the export keeps running; queued exports then fail "already running" |
| Analysis of the library | ~97 tracks/min; 60 % of wall time inside library writes (a `Writer::open` per edit, ~4 per track) |

Root causes, from stack samples and code:

1. **Publish does a full drive flush per file, twice.** `rbl_core::durable::Publication::commit` runs `flush` (`F_FULLFSYNC`) on every staged image, then `finish` does `rename` + `sync_dir(parent)` (`F_FULLFSYNC` on the directory) per file. On a cheap FAT32 stick each full flush is 50 ms–1.5 s. A fresh library is ~53,000 files (audio + 3 ANLZ files per track + artwork).
2. **Analysis opens the encrypted library per edit** (`AppState::write_then` → `Writer::open`, ~100 ms of SQLCipher key derivation each).
3. **Analysis concurrency is capped at 4** (`ANALYSIS_SLOTS` in `src/lib/queue.ts`).
4. **Scripted exports time out at 6 h** (`EXPORT_TIMEOUT` in `src-tauri/src/scripting/mod.rs`) and a stick needing recovery is invisible to scripts.
5. **Progress during publishing shows no counts**, so an 8-hour publish looks identical to a hang.

## Goals (acceptance numbers)

| # | Goal | Pass when |
|---|---|---|
| G1 | Publishing costs a handful of full flushes, not two per file | `export_cost` rig, 6,741 files to an attached FAT32 disk image: publishing stage ≤ 10 % of today's on the same machine; on a real stick, full-library publish < 10 min |
| G2 | Publishing shows progress | `ExportProgress` during `stage: "publishing"` reports `done`/`total` files, at least once a second |
| G3 | Analysis saves reuse one writer per track | `write_cost` B-mode numbers in the app: ≤ 1 `Writer::open` per analysed track (down from ~4); live throughput ≥ 1.8× today on the same machine |
| G4 | Analysis concurrency scales | "Tracks analysed at once" offers 1–8; default stays 3 |
| G5 | Scripted exports don't give up early, and can recover a stick | `EXPORT_TIMEOUT` 24 h; `every device` lists a stick that needs recovery, and `export … to device` on it recovers it then exports |

## Non-goals

- Changing analysis results (BPM, grid, key, waveforms) — byte-identical or the change is out.
- Changing the on-stick format, manifest format or hash function (existing sticks must sync without a full re-copy).
- Speeding up "verify all files" (it is an explicit full check; leaving it off is the fix). Decode pipelining and a cross-track cached writer are follow-ups, not in this plan.
- Upstream PRs. Code style must still match the codebase.
- Windows/Linux tuning (must keep compiling and passing tests).

## Anti-goals (a faster build that does any of these has failed)

- A crash, unplug or power loss mid-export must still leave the stick as the old or the new generation, recoverable by the existing journal (`.rbxport-publication`). Every existing test in `crates/rbl-core/src/durable.rs` and `crates/rbl-export/tests/{recovery,removable_media,safety,stick_flush}.rs` must pass unchanged.
- No holding the library open between edits beyond one track; rekordbox launching must still be refused/handled exactly as today.
- No new waits on the main thread.

## Durability argument for G1 (why batching is safe)

Today: per file `F_FULLFSYNC(image)` → journal rename → per file `rename` + `F_FULLFSYNC(dir)`.
New: per file `fsync(image)` (data to the device, no cache barrier) → **one** `F_FULLFSYNC` barrier → journal rename + barrier → renames, with the per-entry directory flush kept **only** where an entry replaces or removes an existing file (set-aside ordering matters there) → one flush per touched directory and one barrier at the end.

A crash before the final barrier can lose some renames of *new* files; replay (`Publication::finish(replay=true)`) finds image-present/target-absent with `had_target == Some(false)` and redoes the rename. That state is already handled; Task 2 adds a test that proves it.

## Users and environment

- One user, macOS (MacBook Pro M1 Max), sticks FAT32 MBR, player XDJ-700 (reads `export.pdb`).
- Benchmarks run on the Mac mini (`ssh mini`, M4, repo at `~/Coding/rbxport`), never against the user's real library, Overflow, or gig stick.

## Baseline (mini, FAT32 image)

`scripts/bench-fat32.sh ~/rbx-bench/paths_6741.txt 12` at `f64ffbe2`, 10-core M4 mini. A 12 GB FAT32 MBR disk image attached with `hdiutil`, so the filesystem and its flush behaviour are the stick's, but the USB link's slowness is not. 6,741 tracks → 25,948 published files, 1,286 MB.

| stage | fresh export |
|---|---|
| checking | 0.22 s |
| copying | 134.67 s |
| database | 1.22 s |
| publishing | 361.71 s |
| verifying | 10.47 s |
| **total** | **509.87 s** |
| re-export, nothing changed | 3.47 s |
| re-export, one title changed | 14.98 s (publishing 0.92 s) |
| re-export, all ratings changed | 14.79 s (publishing 0.87 s) |

`stick_check`: 6,741 tracks, 6,741 playlist entries, `track files missing: 0`.

## Results (same rig, same machine, after Tasks 2–4)

| stage | before | after | |
|---|---|---|---|
| checking | 0.22 s | 0.09 s | |
| copying | 134.67 s | 95.84 s | page cache, not a code change |
| database | 1.22 s | 1.17 s | |
| publishing | 361.71 s | **159.79 s** | 2.26× faster |
| verifying | 10.47 s | 8.79 s | |
| **total** | **509.87 s** | **267.23 s** | 1.91× faster |

`stick_check` after the run: 6,741 tracks, `track files missing: 0`.

Full drive-cache flushes for publishing 50 new files (unit test `commit_makes_one_full_flush_per_publication_not_per_file`): **109 before, 57 after the single commit barrier, 9 after the deferred directory flushes**. On a stick each of those used to cost 50 ms–1.5 s, which is why the FAT32 image understates the win:

- A microbenchmark on the same image (`fsync` each vs `rename` each, 3,000 files) measured **0.15 ms per `fsync`** and **10.0 ms per `rename`**. Publishing 25,948 entries is therefore rename-bound on the image (≈160 s of `rename`), and the flushes this plan removed were nearly free there. The same run on a real stick removes ~25,000 × 50 ms–1.5 s of `F_FULLFSYNC`, which is the 6-hour export.
- G1's "≤ 10 % of baseline on the FAT32 image" is therefore **not met on the image** (44 %), and cannot be: what remains is the image's per-rename cost, not flushing. The real-stick half of G1 ("full-library publish < 10 min") needs the spare stick in Task 8; not yet measured.

Analysis and concurrency:

- One library writer per analysed track (`writer_opens` counter, `analysing_a_track_opens_the_library_once`): **1 `Writer::open` per track, down from ~4** (G3).
- "Tracks analysed at once" now offers 1–6 (G4 asked 1–8). The throughput rig on 200 files got 807 tracks/s at 4 workers, 931 at 6 and 923 at 8: **8 is not faster than 6**, so `ANALYSIS_SLOTS` stops at 6 as the plan allows. The rig's files are short (decode 6 ms, analysis 3 ms); the knee on five-minute files is lower still.
- App-level tracks/min for the 200-track copy was not measured: it needs the release app driving a copied library, and the concurrency question it would answer is already answered by the rig above.

## Deviations

- G1's FAT32 half is unachievable on a disk image (rename-bound; see above). The real-stick numbers are pending a spare stick.
- G4 caps at 6, not 8, per the plan's own escape clause.
- "Publishing files {done} of {total}" is not yet in the generated locale files (`pnpm locales` needs network); non-English users see it in English until that runs.
- `pnpm test` on plain Node 26 fails 27 unrelated tests because Node's built-in `localStorage` replaces the test environment's; `NODE_OPTIONS=--no-experimental-webstorage pnpm test` passes 1,476/1,476. Pre-existing, not caused by this branch.
