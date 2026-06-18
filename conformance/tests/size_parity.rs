//! Size-parity gate: draco-oxide encoded size vs Google C++ for the same mesh.
//!
//! Prints a full ratio table, then asserts every case stays under
//! [`conformance::SIZE_SHIP_CEILING`]. This is EXPECTED TO FAIL at high
//! quantization bit-depths today (the symbol-scheme-selection bug documented in
//! `KNOWN_ISSUES.md`) — the test turns green when that fix lands. The qp11→qp14
//! growth ratio within each encoder is the cleanest signal: Google grows ~1.5x,
//! draco-oxide ~2x+.

use conformance::*;

#[test]
fn size_parity_full_mesh() {
    let cl = 7;
    let qps = [11u32, 14];
    let mut failures = Vec::new();

    println!(
        "\n{:<16} {:>4} {:>11} {:>11} {:>8}",
        "mesh", "qp", "google_B", "oxide_B", "ratio"
    );
    println!("{}", "-".repeat(54));

    for mesh in mesh_corpus() {
        let obj = mesh_dir().join(mesh);
        for &qp in &qps {
            let g = google_encode(&obj, qp, cl, false).len();
            match oxide_encode(&obj, qp as u8) {
                Ok(o) => {
                    let ratio = o.len() as f64 / g as f64;
                    let flag = if ratio > SIZE_SHIP_CEILING {
                        " <-- over ceiling"
                    } else if ratio > SIZE_GOAL {
                        " (over goal)"
                    } else {
                        ""
                    };
                    println!(
                        "{:<16} {:>4} {:>11} {:>11} {:>7.2}x{}",
                        mesh,
                        qp,
                        g,
                        o.len(),
                        ratio,
                        flag
                    );
                    if ratio > SIZE_SHIP_CEILING {
                        failures.push(format!(
                            "{mesh} qp{qp}: {ratio:.2}x > {SIZE_SHIP_CEILING:.1}x ceiling"
                        ));
                    }
                }
                Err(e) => {
                    println!("{mesh:<16} {qp:>4} {g:>11} {:>11}  ERR: {e}", "-");
                    failures.push(format!("{mesh} qp{qp}: oxide encode failed: {e}"));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "\nSize-parity gate failures (expected until the scheme-selection fix lands):\n  {}",
        failures.join("\n  ")
    );
}
