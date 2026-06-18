//! Byte-identity gate: draco-oxide output vs Google C++, exact bytes.
//!
//! The end goal of the fork is byte-identical output (Draco is deterministic).
//! `torus` is fully there (valence connectivity + position attribute + entropy);
//! this locks it in. The progress report tracks the others toward parity.

use conformance::*;

const QP: u32 = 11;
const CL: u32 = 7;

/// Hard gate: torus must stay byte-identical to Google's encoder.
#[test]
fn torus_byte_identical_to_google() {
    let obj = mesh_dir().join("torus.obj");
    let google = google_encode(&obj, QP, CL, false);
    let oxide = oxide_encode(&obj, QP as u8).expect("oxide encode");
    assert!(
        oxide == google,
        "torus must be byte-identical to Google ({} B oxide vs {} B google); \
         first diff at {:?}",
        oxide.len(),
        google.len(),
        (0..oxide.len().min(google.len())).find(|&i| oxide[i] != google[i]),
    );
}

/// Informational: per-mesh first-divergence offset (run with --nocapture).
/// Always passes — tracks progress toward full byte-identity.
#[test]
fn byte_identity_progress_report() {
    println!("\nbyte-identity vs Google (qp{QP}/cl{CL}):");
    for mesh in mesh_corpus() {
        let obj = mesh_dir().join(mesh);
        let google = google_encode(&obj, QP, CL, false);
        let oxide = match oxide_encode(&obj, QP as u8) {
            Ok(b) => b,
            Err(e) => {
                println!("  {mesh:<16} encode ERR: {e}");
                continue;
            }
        };
        let status = if oxide == google {
            "*** BYTE IDENTICAL ***".to_string()
        } else {
            let min = oxide.len().min(google.len());
            match (0..min).find(|&i| oxide[i] != google[i]) {
                Some(o) => format!(
                    "diverge @ byte {o} (oxide {} B, google {} B)",
                    oxide.len(),
                    google.len()
                ),
                None => format!(
                    "prefix-equal, lengths differ (oxide {} B, google {} B)",
                    oxide.len(),
                    google.len()
                ),
            }
        };
        println!("  {mesh:<16} {status}");
    }
}
