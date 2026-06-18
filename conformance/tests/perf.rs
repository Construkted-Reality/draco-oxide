//! Perf baseline report (non-gating for now).
//!
//! Prints draco-oxide in-process encode time vs Google's CLI encode time on
//! the largest fixture. NOTE: the Google number includes process-startup +
//! file-IO overhead, so it FLATTERS draco-oxide — treat it as indicative only.
//! The accurate apples-to-apples perf comparison (in-process C++ via the cxx
//! bridge, and criterion benches) lands once the cxx link is fixed; see
//! `benches/codec.rs` for the criterion harness on the Rust side.

//! ⚠️ This test runs under `cargo test` = a **debug, unoptimized** build, where
//! Rust is routinely 10–40× slower than release. DO NOT read these numbers as
//! the perf gate. The real perf-parity gate is the **release** criterion bench
//! (`cargo bench -p conformance`). This report exists only for a quick relative
//! sanity check during development.

use conformance::*;
use std::time::{Duration, Instant};

fn min_duration(iters: u32, mut f: impl FnMut()) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..iters {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed());
    }
    best
}

#[test]
fn perf_baseline_report() {
    let obj = mesh_dir().join("bunny.obj");
    let qp = 11u8;
    let iters = 5;

    let mesh = load_mesh(&obj); // parse excluded from timing
    let oxide = min_duration(iters, || {
        let _ = oxide_encode_mesh(mesh.clone(), qp).expect("oxide encode");
    });
    let google = min_duration(iters, || {
        let _ = google_encode(&obj, qp as u32, 7, false);
    });

    let ratio = oxide.as_secs_f64() / google.as_secs_f64();
    println!("\nperf baseline (bunny, qp{qp}, min of {iters}) [DEBUG BUILD - not the gate]:");
    println!("  draco-oxide encode (in-process, debug): {:>8.2?}", oxide);
    println!("  google encode (CLI + process overhead): {:>8.2?}", google);
    println!(
        "  ratio oxide/google: {ratio:.2}x  (debug build + google CLI overhead; NOT a real measure)"
    );
}
