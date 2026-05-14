//! Encodes a positions-only OBJ via our encoder and dumps the bytes.
//!
//! Used for byte-level comparison against Google's reference encoder
//! output (see tests/data/google_fixtures/).
//!
//!     cargo run --example dump_encoded -- path/to/input.obj

use draco_oxide::io::obj::load_obj;
use draco_oxide::prelude::{
    encode, AttributeDomain, AttributeType, MeshBuilder, NdVector, PointIdx,
};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dump_encoded <input.obj>");
    let mesh = load_obj(&path).expect("load obj");

    // Strip non-position attributes for comparable output to Google's
    // `--skip NORMAL --skip TEX_COORD --skip GENERIC`.
    let pos = mesh
        .get_attributes()
        .iter()
        .find(|a| a.get_attribute_type() == AttributeType::Position)
        .unwrap();
    let mut data: Vec<NdVector<3, f32>> = Vec::with_capacity(pos.len());
    for i in 0..pos.len() {
        data.push(pos.get::<NdVector<3, f32>, 3>(PointIdx::from(i)));
    }
    let faces: Vec<[usize; 3]> = mesh
        .get_faces()
        .iter()
        .map(|f| [usize::from(f[0]), usize::from(f[1]), usize::from(f[2])])
        .collect();

    let mut builder = MeshBuilder::new();
    builder.add_attribute::<NdVector<3, f32>, 3>(
        data,
        AttributeType::Position,
        AttributeDomain::Position,
        Vec::new(),
    );
    builder.set_connectivity_attribute(faces);
    let mesh = builder.build().expect("build");

    let mut buf = Vec::new();
    encode::encode(
        mesh,
        &mut buf,
        <encode::Config as draco_oxide::prelude::ConfigType>::default(),
    )
    .expect("encode");

    eprintln!("our encoder: {} bytes", buf.len());
    for (i, chunk) in buf.chunks(16).enumerate() {
        eprint!("{:08x}: ", i * 16);
        for b in chunk {
            eprint!("{:02x} ", b);
        }
        eprintln!();
    }
}
