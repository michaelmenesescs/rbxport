# rbxport Fast Export and Analysis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a full-library USB export publish in minutes instead of hours, show publish progress, cut analysis library-write overhead, and fix the scripting failures seen building the gig stick.

**Architecture:** Export durability moves from "full drive flush per file" to barriers: plain `fsync` per staged file, one `F_FULLFSYNC` before the journal, and one flush per touched directory after the renames, keeping per-entry ordering only where an existing file is replaced. Analysis wraps each track's save in a writer session so all its library edits share one `Writer`. Small, separate tasks widen analysis concurrency and fix the AppleScript export timeout and device listing.

**Tech Stack:** Rust 1.99.0 (pinned in `rust-toolchain.toml`), Tauri 2, SQLCipher via `rbl-db`, React/TypeScript + vitest for the UI, macOS `hdiutil` disk images for FAT32 tests.

**Spec:** `docs/superpowers/specs/2026-10-22-performance-prd.md` (read it first). Background evidence and anti-goals: `docs/superpowers/specs/2026-10-21-performance-design.md`.

## Global Constraints

- Repo: `~/Desktop/Coding/rbxport` (MacBook) / `~/Coding/rbxport` (Mac mini). Branch `perf/analysis-export`. Push only to `origin` = `github.com/michaelmenesescs/rbxport`. Never add an upstream remote, never open PRs against `chrisle/rbxport`.
- **Never touch real data:** not `~/Library/Pioneer/rekordbox/master.db`, not `/Volumes/Overflow`, not `/Volumes/Lexar`. Tests use fixtures (`rbl_db::fixture`), temp dirs, and `hdiutil` images. Benchmarks against a library use the copy at `~/rbx-bench/lib/master.db` on the Mac mini.
- Heavy builds and benchmarks run on the Mac mini: `ssh mini`, then `cd ~/Coding/rbxport && git pull`.
- Workspace lints are errors: no `unwrap`/`expect`/`panic` in non-test code, `clippy::perf` denied, `print_stdout` denied. No `.sync_all(`/`.sync_data(` anywhere in `rbl-export`, `rbl-onelibrary`, `rbl-audio`, `rbl-pdb`, `rbl-anlz`, `rbl-core` sources (enforced by `crates/rbl-export/tests/stick_flush.rs`): go through `rbl_core::durable`.
- Every task must leave these green before its commit:
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test -p rbl-core` and `cargo test -p rbl-export`
  - for tasks touching `src-tauri`: `cargo test -p rbxport --lib`
  - for tasks touching `src/`: `pnpm typecheck && pnpm lint && pnpm test`
- Existing durability tests must pass **unchanged** (do not edit an existing test to make it pass; if one fails, the change is wrong).
- Match surrounding style: doc comments explain *why*, `[OBS …]` tags for observed facts, one-line `tracing::debug!` for timings.
- Commit messages end with:
  ```
  Co-Authored-By: Hermes <noreply@local>
  ```

## Review Focus

1. **Unplug during publish of a fresh export** (all targets new): after replay the stick must hold the complete new generation. Pinned by Task 3's `deferred_new_file_renames_replay_after_a_lost_directory_flush`.
2. **Unplug during publish of a resync that replaces databases** (`export.pdb` etc. exist): old or new generation, never a mix with a missing database. Pinned by keeping per-entry flushes for replacements and by the existing `every_retained_publication_move_boundary_can_replay_or_roll_back_twice` (unchanged).
3. **exFAT stick** (`F_FULLFSYNC` → ENOTSUP): `barrier` must fall back like `flush` does, not fail the export. Pinned by Task 2's `a_barrier_the_filesystem_lacks_falls_back_to_fsync`.
4. **rekordbox launches mid-analysis**: the session writer must not outlive one track. Pinned by Task 5's `a_writer_session_ends_with_its_track`.
5. **A stick left needing recovery**: scripts must see it and `export` must recover it (the gig-night failure). Pinned by Task 7's `a_stick_with_a_publication_journal_is_listed`.

---

### Task 1: FAT32 publish benchmark and stick checker

**Files:**
- Uses as is: `crates/rbl-export/examples/export_cost.rs` (already prints time per stage)
- Add (already written, uncommitted): `crates/rbl-pdb/examples/stick_check.rs`
- Create: `scripts/bench-fat32.sh`
- Modify: `docs/superpowers/specs/2026-10-22-performance-prd.md` (append a "Baseline" table)

**Interfaces:**
- Produces: `scripts/bench-fat32.sh <list-of-paths.txt> [size_gb]` → creates a FAT32 MBR image with `hdiutil create -size <size_gb>g -fs "MS-DOS FAT32" -layout MBRSPUD -volname BENCH`, attaches it, runs `cargo run --release -p rbl-export --example export_cost -- <list> /Volumes/BENCH/out`, then `stick_check /Volumes/BENCH/out`, then detaches. Later tasks re-run it to quote numbers.

- [ ] **Step 1:** Write `scripts/bench-fat32.sh` (bash, `set -euo pipefail`, trap that detaches the image on exit). Default `size_gb` 8.
- [ ] **Step 2:** On the Mac mini, build a list of 2,000 real small audio files (any MP3s under 4 MB on the mini; never Overflow) into `~/rbx-bench/list2000.txt`.
- [ ] **Step 3:** Run `scripts/bench-fat32.sh ~/rbx-bench/list2000.txt`. Expected: export completes, `stick_check` prints `2000 tracks … track files missing: 0`. Record the stage table (copying / verifying / database / publishing seconds).
- [ ] **Step 4:** Append the baseline table to the PRD under `## Baseline (mini, FAT32 image)`.
- [ ] **Step 5:** Commit: `git add scripts/bench-fat32.sh crates/rbl-pdb/examples/stick_check.rs docs/ && git commit -m "bench: FAT32 image publish benchmark and stick checker"`

### Task 2: Flush primitives and a single barrier in `Publication::commit`

**Files:**
- Modify: `crates/rbl-core/src/durable.rs` (add two functions near `flush`, change `Publication::commit` ~lines 505–557)
- Test: `crates/rbl-core/src/durable.rs` `mod tests`

**Interfaces:**
- Produces:
  - `pub fn flush_data(file: &std::fs::File) -> std::io::Result<()>` — plain `fsync` (`rustix::fs::fsync`), no drive-cache barrier. Doc: pushes data to the device; a later `barrier` makes it durable.
  - `pub fn barrier(on: &Path) -> std::io::Result<()>` — opens `on` (file or directory) and calls `flush` on it (`F_FULLFSYNC` flushes the whole drive's cache on Apple platforms); treats `unsupported` the same way `sync_dir` does (falls back to `fsync`, then ignores only ENOTSUP).
- Consumes: existing `flush`, `flush_with`, `unsupported`, `sync_dir`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_barrier_the_filesystem_lacks_falls_back_to_fsync() {
    let dir = tempfile::tempdir().unwrap();
    // barrier on a directory and on a file both succeed on the test volume
    barrier(dir.path()).unwrap();
    let file = dir.path().join("f");
    std::fs::write(&file, b"x").unwrap();
    barrier(&file).unwrap();
}

#[test]
fn commit_makes_one_full_flush_per_publication_not_per_file() {
    // FULL_FLUSHES is a #[cfg(test)] thread_local counter incremented in `flush`.
    let root = tempfile::tempdir().unwrap();
    let publication = Publication::new(root.path(), ".rbxport-publication").unwrap();
    let paths: Vec<PathBuf> = (0..50).map(|i| PathBuf::from(format!("Contents/a/{i}.mp3"))).collect();
    for p in &paths {
        let staged = publication.stage().join(p);
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"audio").unwrap();
    }
    reset_full_flushes();
    publication.commit(&paths).unwrap();
    assert!(full_flushes() <= 12, "got {}", full_flushes());
    for p in &paths { assert!(root.path().join(p).is_file()); }
}
```

- [ ] **Step 2:** Run `cargo test -p rbl-core durable::tests::commit_makes_one_full_flush` — expect FAIL (counter/functions missing, then count ≈ 150).
- [ ] **Step 3:** Implement `flush_data`, `barrier`, and the `#[cfg(test)]` counter (`thread_local! { static FULL_FLUSHES: Cell<usize> }`, `fn reset_full_flushes()`, `fn full_flushes() -> usize`, incremented inside `flush`). In `commit`: replace the per-image `flush(&file)` with `flush_data(&file)`; keep the per-directory `sync_dir` loop over staged directories but change it to `flush_data` on each directory handle (ignore `unsupported`); then call `barrier(self.stage.path())` once **before** `write(publication.json)`. Leave the journal rename + `sync_dir(&self.root)` as is.
- [ ] **Step 4:** Run `cargo test -p rbl-core` — all pass, including every pre-existing durable test. The ≤ 12 bound will still fail until Task 3 (finish's per-file flushes); mark the assertion with the count measured after this step in a comment, and tighten it in Task 3.
- [ ] **Step 5:** Run `cargo test -p rbl-export` and `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] **Step 6:** Commit `perf(durable): one drive barrier for staged images instead of a full flush per file`.

### Task 3: Deferred directory flushes in `Publication::finish`, with progress

**Files:**
- Modify: `crates/rbl-core/src/durable.rs` — `finish` (~696–866), `commit`, `commit_retained`, `finalize_at`
- Test: same file `mod tests`

**Interfaces:**
- Produces:
  - `pub fn commit_observed(&self, paths: &[PathBuf], observe: &mut dyn FnMut(usize, usize)) -> std::io::Result<()>` and `pub fn commit_retained_observed(&self, paths: &[PathBuf], observe: &mut dyn FnMut(usize, usize)) -> std::io::Result<()>`; `observe(done, total)` is called after each entry is published (rate-limit is the caller's job). Existing `commit`/`commit_retained` become wrappers passing `&mut |_, _| {}`.
  - `fn finish(root, journal, replay, observe: &mut dyn FnMut(usize, usize))` (private).
- Consumes: Task 2's `barrier`, `flush_data`.

Rule implemented in `finish`'s per-entry `publish` closure:
- entry is a **relocation** (`moves`), or **replaces** an existing target (set-aside to `.previous`), or is a **deletion**: keep today's `sync_dir` calls exactly.
- entry is a **new file** (`present` and target absent): `rename` with no `sync_dir`; record `target.parent()` (and any directory `create_dir_all` had to create, plus its parent) in a `BTreeSet<PathBuf>` of touched directories. Create missing parents with `std::fs::create_dir_all` (not `durable::create_dir_all`) for these.
- After the loop: `sync_dir` each touched directory, deepest first, then `barrier(root)`, then the existing `.verification-required` check / `retire`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn deferred_new_file_renames_replay_after_a_lost_directory_flush() {
    // Simulate a crash where some new-file renames were not durable: after commit,
    // move three published files back into the journal, recreate the journal record,
    // then Publication::recover must put them back and leave no journal.
    // (Build the journal by calling commit_retained, so `.verification-required`
    // keeps the journal; then move files back; then Publication::finalize_pending.)
    // Assert: all 50 targets exist with their bytes; root has no `.rbxport-publication`.
}

#[test]
fn publishing_reports_progress_for_every_entry() {
    // commit_observed over 20 new files; collect (done,total) pairs.
    // Assert last == (20, 20) and done is non-decreasing.
}
```
Tighten Task 2's bound: `assert!(full_flushes() <= 12)` for 50 new files across one directory.

- [ ] **Step 2:** Run `cargo test -p rbl-core durable` — new tests FAIL.
- [ ] **Step 3:** Implement per the rule above. `recover`/`finalize_pending`/`rollback_*` pass a no-op observer.
- [ ] **Step 4:** Run `cargo test -p rbl-core`, `cargo test -p rbl-export` (especially `--test recovery --test removable_media --test safety --test stick_flush`). All pre-existing tests pass unchanged.
- [ ] **Step 5:** Run `scripts/bench-fat32.sh ~/rbx-bench/list2000.txt` on the mini. Expected: publishing stage ≤ 10 % of the Task 1 baseline. Record in the PRD table.
- [ ] **Step 6:** Commit `perf(durable): flush directories once after publishing new files`.

### Task 4: Publishing progress in the export and the UI

**Files:**
- Modify: `crates/rbl-export/src/lib.rs` — the `publication.commit_retained(&files)` call (~line 1990) and `publication.commit(&files)` (~line 2198)
- Modify: the Sync Manager/status text that renders `stage == "publishing"` (find with `rg '"publishing"' src`)
- Test: `crates/rbl-export/tests/export.rs` (new test), and the UI component's existing vitest file

**Interfaces:**
- Consumes: Task 3's `commit_retained_observed`, `commit_observed`.
- Produces: `ExportProgress { stage: "publishing", done, total, unit: "files", .. }` emitted at most every 250 ms and once at `done == total`.

- [ ] **Step 1:** Failing test `an_export_reports_publishing_progress_in_files`: export 5 tracks to a temp dir with `export_with_options`, capture progress; assert at least one event with `stage == "publishing" && unit == "files" && total > 0` and the last such event has `done == total`.
- [ ] **Step 2:** Run `cargo test -p rbl-export --test export an_export_reports_publishing_progress` — FAIL.
- [ ] **Step 3:** Call the observed variants with a closure that throttles on `Instant` (250 ms) and emits the event above.
- [ ] **Step 4:** UI: when `stage === "publishing"` and `total > 0`, show `Publishing files ${done} of ${total}` (same component and style as the `copying` line). Add a vitest case for that string.
- [ ] **Step 5:** `cargo test -p rbl-export`, `pnpm typecheck && pnpm lint && pnpm test` — pass.
- [ ] **Step 6:** Commit `feat(export): show publishing progress in files`.

### Task 5: One writer per analysed track

**Files:**
- Modify: `src-tauri/src/state.rs` — `AppState` struct (~line 24), `new`/`with_backups` (~179), `write_then` (~284–316)
- Modify: `src-tauri/src/analysis.rs` — `analyse_and_save` (~205–300), `save_analysis_files` (~452–499), `analyse_key_only` (~509)
- Test: `src-tauri/src/analysis.rs` `mod tests`

**Interfaces:**
- Produces on `AppState`:
  - field `session: parking_lot::Mutex<Option<rbl_db::write::Writer>>`
  - field `writer_opens: std::sync::atomic::AtomicU64`, and `pub(crate) fn writer_opens(&self) -> u64`
  - `pub fn writer_session<T>(&self, work: impl FnOnce() -> T) -> T` — takes `edit_gate` (reentrant), runs `work`, then sets `session` to `None` (dropping the writer) even if `work` returns an error. Inside the session, `write_then` opens the writer on first use, stores it in `session`, and reuses it for later edits; outside a session `write_then` behaves exactly as today.
- Consumes: nothing from export tasks.

- [ ] **Step 1: Write the failing tests** (reuse the fixture setup from `analysing_a_fixture_track_writes_its_files_and_registers_them`; factor it into a `fn fixture_state(dir: &Path) -> (AppState, Arc<rbl_index::Library>, PathBuf /* share */, crate::grid::GridEditor)` helper in the test module)

```rust
#[test]
fn analysing_a_track_opens_the_library_once() {
    let dir = tempfile::tempdir().unwrap();
    let (state, library, share, editor) = fixture_state(dir.path());
    let before = state.writer_opens();
    analyse_and_save(&state, &library, &share, &track_id(0), rbl_analysis::AnalysisPreset::Rbxport, &AnalysisSettings::default(), &editor, None).unwrap();
    assert_eq!(state.writer_opens() - before, 1);
}

#[test]
fn a_writer_session_ends_with_its_track() {
    // after analyse_and_save returns (Ok or Err), state.session is None
    // and a fresh write opens a new writer (writer_opens increments).
}
```

- [ ] **Step 2:** `cargo test -p rbxport --lib analysis::tests::analysing_a_track_opens_the_library_once` — FAIL (today ≈ 3–4).
- [ ] **Step 3:** Implement the fields and `writer_session`; in `write_then`, increment `writer_opens` on every `Writer::open`; reuse `session` when `Some`. Wrap the save half of `analyse_and_save` (from `let _edit_guard = state.edit_gate.lock();` to the end) and the body of `analyse_key_only` after its guard in `state.writer_session(|| …)`. `FileJournal::prepare`/`publish`/`commit` stay where they are, inside the session.
- [ ] **Step 4:** If `FileJournal::prepare`'s read-only open fails or blocks while the session writer is open, have `prepare` take the needed values from the caller instead of opening the library (pass them as arguments), keeping its recovery semantics; do not drop the session around it.
- [ ] **Step 5:** `cargo test -p rbxport --lib` — all pass, including `analysing_a_fixture_track_writes_its_files_and_registers_them` and the file-journal recovery tests unchanged.
- [ ] **Step 6:** On the mini: `cargo run --release -p rbl-db --example write_cost -- ~/rbx-bench/lib/master.db 200` (unchanged rig, for reference) and a release build of the app analysing the same 200-track playlist from a copied library before/after; record tracks/min in the PRD.
- [ ] **Step 7:** Commit `perf(analysis): one library writer per analysed track`.

### Task 6: Up to 8 tracks analysed at once

**Files:**
- Modify: `src/lib/queue.ts:19-20` — `ANALYSIS_SLOTS = [1, 2, 3, 4, 5, 6, 7, 8] as const`; keep `SLOTS = 3`; update the doc comment with the mini measurement.
- Modify: the select's options in `src/views/settings/AnalysisPane.tsx` if they are hard-coded
- Test: the preferences test that covers `oneOfNumber(analysis.concurrentTracks, …)` (find with `rg concurrentTracks src --glob '*.test.*'`)

- [ ] **Step 1:** Failing test: stored preference `concurrentTracks: 8` survives `preferences` parsing; `9` falls back to `3`.
- [ ] **Step 2:** Run `pnpm test` — FAIL.
- [ ] **Step 3:** Change `ANALYSIS_SLOTS`.
- [ ] **Step 4:** On the mini after Task 5: analyse the 200-track copy at 4, 6 and 8; put the three tracks/min numbers in the `queue.ts` doc comment. If 8 is not faster than 6, cap `ANALYSIS_SLOTS` at 6.
- [ ] **Step 5:** `pnpm typecheck && pnpm lint && pnpm test`.
- [ ] **Step 6:** Commit `feat(analysis): allow up to 8 tracks analysed at once`.

### Task 7: Scripted exports: 24 h timeout, and sticks needing recovery

**Files:**
- Modify: `src-tauri/src/scripting/mod.rs:58` — `EXPORT_TIMEOUT = Duration::from_secs(24 * 60 * 60)`
- Investigate + modify: `rbl_devices::list()` (`crates/rbl-devices/src`) and/or the window's script `devices` answer, and the `export` script handler (`src-tauri/src/scripting/cocoa.rs:~1425` → window handler in `src/`), and `src-tauri/src/commands.rs:1438` (`recovery_pending` refusal)
- Test: `crates/rbl-devices` tests (disk image or temp dir), `src-tauri` scripting tests

**Interfaces:**
- Produces: a stick whose root holds `.rbxport-publication` appears in `rbl_devices::list()` (and so in AppleScript `every device`); the script `export` on it runs the normal export, which already starts with `recover(destination)` (`rbl-export/src/lib.rs:~967`).

- [ ] **Step 1:** Reproduce: make a FAT32 `hdiutil` image, export 3 tracks to it, then create an empty `.rbxport-publication/publication.json` with `[]` and `.verification-required`; check whether `rbl_devices::list()` returns it. Write the result in the commit message.
- [ ] **Step 2:** Failing test `a_stick_with_a_publication_journal_is_listed` in the crate where the filter lives.
- [ ] **Step 3:** Remove the filter (or the early refusal) so listing and exporting are allowed; browsing and ejecting keep refusing with the existing message (`commands.rs:1438`, `:2000`).
- [ ] **Step 4:** Change `EXPORT_TIMEOUT`; update its doc comment ("a full library to a slow stick took over 6 h on 2026-10-21").
- [ ] **Step 5:** `cargo test -p rbl-devices`, `cargo test -p rbxport --lib`, clippy.
- [ ] **Step 6:** Commit `fix(scripting): export to a stick needing recovery, 24 h export timeout`.

### Task 8: End-to-end check on a real stick, then release build

**Files:** none new. Updates the PRD results table.

- [ ] **Step 1:** On the mini: `pnpm tauri build` (release). Install it **on the mini only**.
- [ ] **Step 2:** Format a spare stick (not the Lexar): `diskutil eraseDisk FAT32 BENCHSTICK MBRFormat diskN` — confirm `diskN` with `diskutil list external` first.
- [ ] **Step 3:** Export the 2,000-file list from a copied library to it via the app; time copy and publish; run `stick_check /Volumes/BENCHSTICK` → `missing: 0`.
- [ ] **Step 4:** Pull the stick mid-publish once, replug, click Sync; confirm it recovers and `stick_check` passes.
- [ ] **Step 5:** Record numbers in the PRD; commit `docs: results for fast publish and analysis`; `git push origin perf/analysis-export`.
- [ ] **Step 6:** Report to the owner: before/after table, and the install path of the built app. Do not install it on the MacBook or touch the Lexar; the owner decides when to switch.
