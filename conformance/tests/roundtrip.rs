//! Self-consistency gate: draco-oxide must decode its own output. Exercises the
//! full encode -> decode path (prediction + transforms + symbol coding), the
//! same path the in-crate glb roundtrip tests use, but over the OBJ corpus so
//! failures localize to a mesh + quantization level.

use conformance::*;

#[test]
fn oxide_self_roundtrip() {
    let mut failures = Vec::new();
    for mesh in mesh_corpus() {
        let obj = mesh_dir().join(mesh);
        for qp in [11u8, 14] {
            let drc = match oxide_encode(&obj, qp) {
                Ok(b) => b,
                Err(e) => {
                    failures.push(format!("{mesh} qp{qp}: encode failed: {e}"));
                    continue;
                }
            };
            match oxide_decode(&drc) {
                Ok(_m) => println!("{mesh} qp{qp}: oxide self-roundtrip OK ({} B)", drc.len()),
                Err(e) => failures.push(format!(
                    "{mesh} qp{qp}: oxide could NOT decode its own output: {e}"
                )),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "\nself-roundtrip failures:\n  {}",
        failures.join("\n  ")
    );
}
