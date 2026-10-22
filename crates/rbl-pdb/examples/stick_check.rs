//! Read-only check of a stick's `export.pdb`: what a Device Library player
//! would list, and whether each track's file is where the row says.
//!
//! `cargo run --release -p rbl-pdb --example stick_check -- /Volumes/STICK`
#![allow(clippy::pedantic, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("the stick's mount point"));
    let bytes = std::fs::read(root.join("PIONEER/rekordbox/export.pdb")).expect("export.pdb");
    let pdb = match rbl_pdb::Pdb::parse(&bytes) {
        Ok(pdb) => pdb,
        Err(e) => { println!("export.pdb does not parse: {e}"); return; }
    };
    let tracks = pdb.table(rbl_pdb::PageType::Tracks).map(|t| pdb.track_rows(t)).unwrap_or_default();
    let nodes = pdb.table(rbl_pdb::PageType::PlaylistTree).map(|t| pdb.playlist_nodes(t)).unwrap_or_default();
    let entries = pdb.table(rbl_pdb::PageType::PlaylistEntries).map(|t| pdb.playlist_entries(t)).unwrap_or_default();
    let missing: Vec<&str> = tracks.iter().map(|t| t.file_path.as_str())
        .filter(|p| !root.join(p.trim_start_matches('/')).is_file()).collect();
    println!("export.pdb {} bytes: {} tracks, {} playlists/folders, {} playlist entries", bytes.len(), tracks.len(), nodes.len(), entries.len());
    println!("track files missing: {}", missing.len());
    for p in missing.iter().take(5) { println!("  {p}"); }
    for t in tracks.iter().take(3) { println!("  sample: {:>5} {:7.2} {}", t.id, f64::from(t.tempo_x100) / 100.0, t.file_path); }
}
