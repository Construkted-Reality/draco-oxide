//! Criterion benches for the Rust-side codec (perf-parity gate, Rust half).
//!
//! Run native release: `cargo bench -p conformance`. The Google-side timing
//! (in-process C++) is added once the `draco_decoder` cxx link is fixed; until
//! then compare against `tests/perf.rs`'s CLI baseline.

use conformance::{load_mesh, mesh_dir, oxide_encode_mesh};
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_encode(c: &mut Criterion) {
    let obj = mesh_dir().join("bunny.obj");
    let mesh = load_mesh(&obj); // parse once, excluded from timing
    for qp in [11u8, 14] {
        c.bench_function(&format!("oxide_encode_bunny_qp{qp}"), |b| {
            b.iter(|| oxide_encode_mesh(mesh.clone(), qp).expect("oxide encode"));
        });
    }
}

criterion_group!(benches, bench_encode);
criterion_main!(benches);
