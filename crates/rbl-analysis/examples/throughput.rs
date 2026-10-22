//! Analysis throughput rig: how long the decode and the DSP take per track,
//! and how many tracks a second a pool of workers gets through.
//!
//! cargo run --release -p rbl-analysis --example throughput -- <list-of-paths.txt> [workers...]
//!
//! Each line of the list is an audio file. Prints the per-track split for a
//! single worker, then the wall-clock rate for each worker count given
//! (default 1 4 8 10).
#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// What one track cost.
struct Cost {
    decode: Duration,
    analyse: Duration,
    secs_of_audio: f64,
}

fn one(path: &std::path::Path, options: &rbl_analysis::AnalysisOptions) -> Option<Cost> {
    let started = Instant::now();
    let audio = rbl_audio::decode_mono(path, Some(1800.0)).ok()?;
    let decode = started.elapsed();
    let started = Instant::now();
    let analysis = rbl_analysis::analyse_with(&audio.samples, audio.sample_rate, *options);
    let analyse = started.elapsed();
    std::hint::black_box(analysis);
    Some(Cost { decode, analyse, secs_of_audio: audio.duration_secs() })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let list = args.next().expect("a file listing one audio path per line");
    let workers: Vec<usize> = {
        let given: Vec<usize> = args.filter_map(|a| a.parse().ok()).collect();
        if given.is_empty() { vec![1, 4, 8, 10] } else { given }
    };
    let paths: Vec<PathBuf> = std::fs::read_to_string(list)
        .expect("readable list")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    let mut options = rbl_analysis::AnalysisPreset::Rbxport.options();
    options.tempo.placement = rbl_analysis::tempo::Placement::Attack;

    // Single worker: the split between decode and DSP.
    let (mut decode, mut analyse, mut audio, mut n) = (Duration::ZERO, Duration::ZERO, 0.0, 0);
    for path in &paths {
        if let Some(cost) = one(path, &options) {
            decode += cost.decode;
            analyse += cost.analyse;
            audio += cost.secs_of_audio;
            n += 1;
        }
    }
    println!(
        "single worker: {n} tracks, {:.1} min of audio | decode {:.0} ms/track, analyse {:.0} ms/track",
        audio / 60.0,
        decode.as_secs_f64() * 1000.0 / f64::from(n),
        analyse.as_secs_f64() * 1000.0 / f64::from(n),
    );

    // A pool of workers pulling from one queue, as the app's slots do.
    for &count in &workers {
        let next = AtomicUsize::new(0);
        let started = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..count {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(i) else { break };
                    std::hint::black_box(one(path, &options));
                });
            }
        });
        let wall = started.elapsed().as_secs_f64();
        println!("{count:>2} workers: {wall:.1} s for {} tracks = {:.2} tracks/s", paths.len(), paths.len() as f64 / wall);
    }
}
