//! Byte-level divergence locator: draco-oxide vs Google C++ for the same mesh.
//!
//! The north star is byte-IDENTICAL output (Draco encoding is deterministic, so
//! there is no reason the two can't match). This tool finds WHERE they first
//! diverge so we can attack one section at a time.
//!
//! Usage (memory-guarded, release):
//! ```
//! systemd-run --user --scope -q -p MemoryMax=4G nice -n15 \
//!   cargo run -p conformance --release --bin bytediff -- [mesh.obj] [qp]
//! ```
//! Defaults: tetrahedron.obj, qp=11. The mesh arg may be a corpus name
//! (e.g. `sphere`) or a path.
//!
//! NOTE: oxide encodes the full mesh; Google here also encodes the full mesh at
//! the same -qp/-qt so the attribute set matches. Header/connectivity come
//! first, so the first-divergence offset usually points at the earliest
//! unmatched stage. Pair with the `DRACO_SCHEME_LOG` instrumentation and the
//! Draco bitstream layout to attribute the offset to a section.

use conformance::*;
use std::path::PathBuf;

fn resolve_mesh(arg: &str) -> PathBuf {
    let p = PathBuf::from(arg);
    if p.exists() {
        p
    } else if arg.ends_with(".obj") {
        mesh_dir().join(arg)
    } else {
        mesh_dir().join(format!("{arg}.obj"))
    }
}

fn hexdump(label: &str, bytes: &[u8], center: usize, radius: usize) {
    let start = center.saturating_sub(radius);
    let end = (center + radius).min(bytes.len());
    print!("  {label:<7} [{start:>5}..{end:<5}] ");
    for (i, b) in bytes[start..end].iter().enumerate() {
        let off = start + i;
        if off == center {
            print!("[{b:02x}]");
        } else {
            print!(" {b:02x} ");
        }
    }
    println!();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mesh_arg = args.get(1).map(|s| s.as_str()).unwrap_or("tetrahedron");
    let qp: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(11);
    let obj = resolve_mesh(mesh_arg);

    println!("\nbyte-diff: {} @ qp{qp} (cl7)\n", obj.display());

    let google = google_encode(&obj, qp, 7, false);
    let oxide = match oxide_encode(&obj, qp as u8) {
        Ok(b) => b,
        Err(e) => {
            println!("oxide encode failed: {e}");
            return;
        }
    };

    println!(
        "sizes: google={} B  oxide={} B  (delta {:+} B, {:.3}x)",
        google.len(),
        oxide.len(),
        oxide.len() as i64 - google.len() as i64,
        oxide.len() as f64 / google.len() as f64
    );

    let min = google.len().min(oxide.len());
    let first_diff = (0..min).find(|&i| google[i] != oxide[i]);

    match first_diff {
        None if google.len() == oxide.len() => {
            println!("\n*** BYTE IDENTICAL ***");
        }
        None => {
            println!(
                "\nidentical for the first {min} bytes; lengths differ \
                (one is a prefix of the other)."
            );
            hexdump("google", &google, min.saturating_sub(1), 16);
            hexdump("oxide", &oxide, min.saturating_sub(1), 16);
        }
        Some(off) => {
            let pct = 100.0 * off as f64 / min as f64;
            println!("\nfirst divergence at byte {off} ({pct:.1}% through the shorter stream):");
            hexdump("google", &google, off, 16);
            hexdump("oxide", &oxide, off, 16);
            // How many bytes match from the start — a low number means we
            // diverge in the header/connectivity; high means deep in attributes.
            println!("\n  matched {off} leading bytes before diverging.");
        }
    }
}
