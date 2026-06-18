//! Profiling driver: tight-loops bunny encode or decode so a sampling profiler
//! (perf) collects enough samples to attribute the Rust vs C++ time gap.
//!
//! Build with debug info, then sample:
//! ```
//! RUSTFLAGS="-C debuginfo=2" cargo build -p conformance --release --bin profile
//! perf record -g --call-graph dwarf -o /tmp/enc.data -- target/release/profile encode 400
//! perf report -i /tmp/enc.data --stdio
//! ```
//! Arg 1: `encode` | `decode` (default encode). Arg 2: iterations.

use conformance::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("encode");
    let iters: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(400);

    let obj = mesh_dir().join("bunny.obj");
    let mesh = load_mesh(&obj);

    match mode {
        "encode" => {
            // encode consumes the mesh, so clone per iter (Mesh::clone will show
            // as its own frame in the profile — discount it).
            let mut sink = 0usize;
            for _ in 0..iters {
                let b = oxide_encode_mesh(mesh.clone(), 11).expect("encode");
                sink = sink.wrapping_add(b.len());
            }
            std::hint::black_box(sink);
        }
        "decode" => {
            let drc = oxide_encode_mesh(mesh.clone(), 11).expect("encode");
            let mut sink = 0usize;
            for _ in 0..iters {
                let m = oxide_decode(&drc).expect("decode");
                sink = sink.wrapping_add(m.get_faces().len());
            }
            std::hint::black_box(sink);
        }
        other => panic!("unknown mode {other:?} (use encode|decode)"),
    }
}
