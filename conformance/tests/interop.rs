//! Interop gate: Google's decoder must be able to decode draco-oxide's output.
//!
//! This is the core product requirement for tileforge — a `.drc` we write must
//! be loadable by any standard Draco decoder. (The reverse direction —
//! draco-oxide decoding Google's output — is covered by draco-oxide's own
//! `google_compat.rs` once its cxx-link issue is resolved; here we use the CLI.)

use conformance::*;

#[test]
fn oxide_output_is_google_decodable() {
    let mut failures = Vec::new();

    for mesh in mesh_corpus() {
        let obj = mesh_dir().join(mesh);
        match oxide_encode(&obj, 11) {
            Ok(drc) => match google_decode(&drc) {
                Ok(obj_text) => {
                    let verts = obj_text.lines().filter(|l| l.starts_with("v ")).count();
                    let faces = obj_text.lines().filter(|l| l.starts_with("f ")).count();
                    println!(
                        "{mesh}: google decoded oxide output OK ({} B -> {verts} v, {faces} f)",
                        drc.len()
                    );
                }
                Err(e) => failures.push(format!(
                    "{mesh}: google could NOT decode oxide output: {}",
                    e.lines().next().unwrap_or("(no stderr)")
                )),
            },
            Err(e) => failures.push(format!("{mesh}: oxide encode failed: {e}")),
        }
    }

    assert!(
        failures.is_empty(),
        "\nInterop gate failures:\n  {}",
        failures.join("\n  ")
    );
}
