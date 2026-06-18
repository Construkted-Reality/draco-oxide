//! Performance + size baseline: draco-oxide vs Google C++ Draco.
//!
//! Run native release, memory-guarded:
//! ```
//! systemd-run --user --scope -q -p MemoryMax=4G nice -n15 \
//!   cargo run -p conformance --release --bin baseline
//! ```
//!
//! Fair codec-only comparison: draco-oxide is timed in-process (parse + IO
//! excluded, min of N iterations); Google is its CLI's self-reported
//! "ms to encode/decode" (also codec-only, IO excluded). Sizes are exact.
//!
//! This is a STARTING-POINT baseline on whatever build profile you run it with —
//! always run `--release` for meaningful perf numbers.

use conformance::*;
use std::time::Instant;

const ITERS: u32 = 9;
const QP: u32 = 11;
const CL: u32 = 7;

/// Google's CLI reports integer milliseconds, so sub-ms meshes read as 0 and
/// the ratio is meaningless. Show "n/a" rather than a bogus "inf".
fn fmt_ratio(oxide_ms: f64, google_ms: f64) -> String {
    if !google_ms.is_finite() || google_ms < 0.5 {
        "n/a".to_string()
    } else {
        format!("{:.1}x", oxide_ms / google_ms)
    }
}

fn min_ms(iters: u32, mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..iters {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

fn main() {
    let profile = if cfg!(debug_assertions) {
        "DEBUG (run --release for real numbers!)"
    } else {
        "release"
    };
    println!("\ndraco-oxide vs Google C++ Draco — baseline  (qp={QP}, cl={CL}, min of {ITERS})");
    println!("build profile: {profile}\n");

    // ---- size + encode time ----
    println!("ENCODE + SIZE");
    println!(
        "{:<14} {:>7} {:>10} {:>10} {:>7} | {:>9} {:>9} {:>7}",
        "mesh", "verts", "google_B", "oxide_B", "size", "g_enc_ms", "o_enc_ms", "enc"
    );
    println!("{}", "-".repeat(82));

    for mesh in mesh_corpus() {
        let obj = mesh_dir().join(mesh);
        let parsed = load_mesh(&obj);
        let verts = parsed
            .get_attributes()
            .first()
            .map(|a| a.len())
            .unwrap_or(0);

        let (g_bytes, g_enc_ms) = google_encode_timed(&obj, QP, CL, false);

        let o_bytes = match oxide_encode_mesh(parsed.clone(), QP as u8) {
            Ok(b) => b,
            Err(e) => {
                println!("{mesh:<14} {verts:>7}   oxide encode ERR: {e}");
                continue;
            }
        };
        let o_enc_ms = min_ms(ITERS, || {
            let _ = oxide_encode_mesh(parsed.clone(), QP as u8);
        });

        let size_ratio = o_bytes.len() as f64 / g_bytes.len() as f64;
        let g_ms = g_enc_ms.unwrap_or(f64::NAN);
        println!(
            "{:<14} {:>7} {:>10} {:>10} {:>6.2}x | {:>9.1} {:>9.2} {:>6}",
            mesh,
            verts,
            g_bytes.len(),
            o_bytes.len(),
            size_ratio,
            g_ms,
            o_enc_ms,
            fmt_ratio(o_enc_ms, g_ms)
        );
    }

    // ---- decode time (each codec on its OWN output, so both decode valid input) ----
    println!("\nDECODE  (each decoder on its own encoder's output)");
    println!(
        "{:<14} {:>9} {:>9} {:>7}  {}",
        "mesh", "g_dec_ms", "o_dec_ms", "dec", "notes"
    );
    println!("{}", "-".repeat(60));

    for mesh in mesh_corpus() {
        let obj = mesh_dir().join(mesh);
        let parsed = load_mesh(&obj);

        // Google decodes Google's bytes.
        let (g_bytes, _) = google_encode_timed(&obj, QP, CL, false);
        let (_g_res, g_dec_ms) = google_decode_timed(&g_bytes);

        // oxide decodes oxide's bytes.
        let o_bytes = match oxide_encode_mesh(parsed, QP as u8) {
            Ok(b) => b,
            Err(e) => {
                println!("{mesh:<14}   oxide encode ERR: {e}");
                continue;
            }
        };
        let mut note = String::new();
        let o_dec_ms = match oxide_decode(&o_bytes) {
            Ok(_) => min_ms(ITERS, || {
                let _ = oxide_decode(&o_bytes);
            }),
            Err(e) => {
                note = format!("oxide decode ERR: {e}");
                f64::NAN
            }
        };

        let g_ms = g_dec_ms.unwrap_or(f64::NAN);
        println!(
            "{:<14} {:>9.2} {:>9.2} {:>6}  {}",
            mesh,
            g_ms,
            o_dec_ms,
            fmt_ratio(o_dec_ms, g_ms),
            note
        );
    }

    println!(
        "\nratios = oxide / google (lower is better; 1.0x = parity). \
         google ms = CLI self-reported codec time (IO excluded)."
    );
}
