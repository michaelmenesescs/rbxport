//! Export timing rig: a fresh export of a list of audio files into an empty
//! folder, then the same export again over it, with the time spent in each
//! stage the progress reports.
//!
//! `cargo run --release -p rbl-export --example export_cost -- <list-of-paths.txt> <dest>`
#![allow(clippy::pedantic, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn run(label: &str, dest: &std::path::Path, tracks: &[rbl_export::SourceTrack], playlists: &[rbl_export::SourcePlaylist]) {
    let mut stages: BTreeMap<&'static str, Duration> = BTreeMap::new();
    let mut current: Option<(&'static str, Instant)> = None;
    let started = Instant::now();
    let report = rbl_export::export_with_options(dest, tracks, playlists, &[], &rbl_export::ExportOptions::default(), &mut |p| {
        if current.map(|(stage, _)| stage) != Some(p.stage) {
            if let Some((stage, since)) = current.take() {
                *stages.entry(stage).or_default() += since.elapsed();
            }
            current = Some((p.stage, Instant::now()));
        }
    })
    .expect("export");
    if let Some((stage, since)) = current {
        *stages.entry(stage).or_default() += since.elapsed();
    }
    let wall = started.elapsed().as_secs_f64();
    let mb = report.bytes_copied as f64 / 1_048_576.0;
    println!("{label}: {wall:.2} s | copied {} ({mb:.0} MB, {:.0} MB/s), reused {}", report.tracks_added + report.tracks_updated, mb / wall, report.reused);
    for (stage, spent) in stages {
        println!("    {stage:<11} {:>7.2} s", spent.as_secs_f64());
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let list = args.next().expect("a file listing one audio path per line");
    let dest = PathBuf::from(args.next().expect("an empty destination folder"));
    let tracks: Vec<rbl_export::SourceTrack> = std::fs::read_to_string(list)
        .expect("readable list")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, path)| {
            let path = PathBuf::from(path);
            rbl_export::SourceTrack {
                id: i as u64 + 1,
                file_size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                title: path.file_stem().unwrap().to_string_lossy().into_owned(),
                artist: format!("Artist {i}"),
                bpm_x100: 12_400,
                duration_sec: 400,
                source_path: path,
                ..Default::default()
            }
        })
        .collect();
    let playlists = vec![rbl_export::SourcePlaylist {
        id: 1,
        name: "bench".to_owned(),
        track_indices: (0..tracks.len()).collect(),
        ..Default::default()
    }];
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).unwrap();
    run("fresh export ", &dest, &tracks, &playlists);
    run("re-export    ", &dest, &tracks, &playlists);
    let mut changed = tracks.clone();
    changed[0].title.push_str(" (edit)");
    run("one title changed", &dest, &changed, &playlists);
    let mut more = changed.clone();
    for track in &mut more { track.rating = 3; }
    run("all ratings changed", &dest, &more, &playlists);
}
