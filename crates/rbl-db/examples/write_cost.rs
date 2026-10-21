//! What saving one analysis costs, against a **copy** of a real library.
//!
//! The app opens a writer per edit and makes about three edits per analysed
//! track (the analysis path, the key row, the analysis itself). This times
//! that, then the same edits through one writer per track, then through one
//! writer for the whole batch.
//!
//! `cargo run --release -p rbl-db --example write_cost -- <copy>/master.db [tracks]`
#![allow(clippy::pedantic, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rbl_db::write::{AnalysisWrite, Writer};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("path to a COPY of master.db"));
    let tracks: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(200);
    if let Ok(real) = rbl_db::detect() {
        assert_ne!(path.canonicalize().ok(), real.master_db.canonicalize().ok(), "refusing the installed library");
    }
    let location = rbl_db::LibraryLocation {
        master_db: path.clone(),
        share_root: path.parent().map(|p| p.join("share")).unwrap_or_default(),
        passphrase: rbl_db::key::derive_password(rbl_db::key::REKORDBOX_DP).expect("standard key"),
        is_real_install: false,
    };
    let backups = std::env::temp_dir().join("rbx-write-cost-backups");
    let open = || {
        let mut writer = Writer::open(location.clone(), &backups).expect("open");
        writer.disable_automatic_backups();
        writer
    };
    let ids: Vec<String> = {
        let writer = open();
        let mut statement = writer.library().connection()
            .prepare("SELECT ID FROM djmdContent WHERE rb_local_deleted = 0 LIMIT ?1").unwrap();
        statement.query_map([tracks as i64], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    println!("{} tracks from {}", ids.len(), path.display());

    let time = |label: &str, cost: Duration| {
        println!("{label:<44} {:>7.1} ms/track", cost.as_secs_f64() * 1000.0 / ids.len() as f64);
    };
    let edits = |writer: &mut Writer, id: &str| {
        let relative = writer.analysis_data_path_for(id).unwrap();
        writer.ensure_detected_key("Am").unwrap();
        writer.set_analysis(id, &AnalysisWrite { bpm_x100: 12_400, key: Some("Am"), analysis_path: &relative, length_sec: None }).unwrap();
    };

    // The pieces of one open.
    let started = Instant::now();
    for _ in &ids { std::hint::black_box(rbl_db::is_rekordbox_running()); }
    time("rekordbox-running check (x1)", started.elapsed());
    let started = Instant::now();
    for _ in &ids { drop(open()); }
    time("writer open (x1)", started.elapsed());

    // As the app does it: a writer per edit.
    let started = Instant::now();
    for id in &ids {
        let relative = open().analysis_data_path_for(id).unwrap();
        open().ensure_detected_key("Am").unwrap();
        open().set_analysis(id, &AnalysisWrite { bpm_x100: 12_400, key: Some("Am"), analysis_path: &relative, length_sec: None }).unwrap();
    }
    time("A. writer per edit (today)", started.elapsed());

    let started = Instant::now();
    for id in &ids { edits(&mut open(), id); }
    time("B. one writer per track", started.elapsed());

    let started = Instant::now();
    let mut writer = open();
    for id in &ids { edits(&mut writer, id); }
    time("C. one writer for the batch", started.elapsed());
}
