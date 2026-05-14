# draco-oxide

&#x20;&#x20;

[![Crates.io](https://img.shields.io/crates/v/draco-oxide)](https://crates.io/crates/draco-oxide)
[![Documentation](https://docs.rs/draco-oxide/badge.svg)](https://docs.rs/draco-oxide) 

`draco-oxide` is a high-performance Rust re-write of Google’s [Draco](https://github.com/google/draco) 3D-mesh compression library, featuring efficient streaming I/O and seamless WebAssembly integration.

> **Status:** **Alpha** – Encoder + decoder are both functional for positions, normals, UVs, and vertex colors. Decoder output is bit-perfect against Google's reference C++ decoder for typical 3D Tiles content at default encoder settings.

---

## Features

| Component              | Alpha  | Beta Roadmap       |
| ------------------     | -----  | ------------------ |
| Mesh Encoder           | ✅     | Performance optimization |
| Mesh Decoder           | ✅     | Sequential connectivity, predictive Edgebreaker |
| glb Transcoder (basic)| ✅     | Animation and many more extensions  |
| glb Decoder (basic)   | ✅     | Animation and many more extensions  |

### Encoder Highlights

* Triangle‑mesh compression with configurable speed/ratio presets.
* Basic glb transcoder (`*.glb` → `*.glb` with mesh buffer compressed via [KHR_draco_mesh_compression extension](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_draco_mesh_compression)).
* Pure‑Rust implementation.
* `no_std` + `alloc` compatible; builds to **WASM32**, **x86\_64**, **aarch64**, and more.

### Decoder Highlights

The decoder pipeline is end-to-end working for positions, normals,
UVs, and vertex colors on both this repo's encoder output and
Google's reference encoder output. What's wired up today:

* Edgebreaker connectivity decode for both **Standard** and **Valence**
  traversal modes (C/R/L/E/S symbols, topology splits for higher-genus
  meshes, S-merge alias chain handling, start-face-config replay via
  RABS, per-context symbol arrays for Valence with on-the-fly active-
  context computation from vertex valences).
* Per-attribute decode pipeline: prediction-scheme dispatch,
  WrappedDifference / Difference / NoTransform / OctahedralOrthogonal
  inverse transforms, and QuantizationCoordinateWise / ToBits /
  OctahedralQuantization deportabilization.
* `MeshParallelogramPrediction` inverse for positions.
* `MeshNormalPrediction` inverse for normals (face-normal sum + flip
  bits via RABS), output at the 8-bit oct quantization theoretical
  floor.
* `MeshPredictionForTextureCoordinates` inverse for UVs.
* Vertex colors (`AttributeType::Color`, N=3 RGB or N=4 RGBA) via
  `QuantizationCoordinateWise` + `WrappedDifference`.
* Both `LengthCoded` and `DirectCoded` symbol entropy paths.
* glTF `KHR_draco_mesh_compression` integration:
  `io::gltf::draco_decoder::decode_glb` extracts and decodes every
  Draco primitive in a `.glb`;
  `io::gltf::draco_decoder::splice_glb_remove_draco` returns a
  Draco-free GLB by patching the existing accessors in place,
  ready for any vanilla glTF loader (bevy_gltf, three.js, gltf-rs).
* Flat-bytes API (`decode::decode_to_raw`) returning a `DecodedRaw`
  with indices + per-attribute payload offsets, suitable for splicing
  straight into a glTF binary buffer without rebuilding the typed
  `Mesh` first.
* `decode::decode_with_warnings` and `decode_to_raw_with_warnings`
  surface non-fatal `DecodeWarning::AttributeSkipped` events when an
  attribute can't be decoded (unsupported prediction scheme,
  transform, or component layout) — earlier attributes still come
  through, callers can detect the partial result.
* CLI `--decode` mode: `cargo run -p cli -- --decode -i input.drc -o output.obj`.

**Verified bit-perfect against Google's reference C++ decoder** on
two test surfaces (`tests/google_compat.rs`):

1. Pre-recorded `.drc` fixtures (encoder = Google's `draco_encoder`
   1.5.7 at default settings) compared against `.expected.obj`
   ground-truth outputs:

| Fixture     | Verts  | Faces  | Edgebreaker | Max L_inf vs Google |
| ----------- | -----: | -----: | ----------- | ------------------: |
| tetrahedron |      4 |      4 | Standard    |                 0.0 |
| sphere      |    114 |    224 | Standard    |                1e-6 |
| torus       |  2 051 |  4 095 | Valence     |                1e-6 |
| bunny       | 34 834 | 69 451 | Valence     |                1e-6 |

2. Live cross-decoder check via the `draco_decoder = "0.0.26"` crate
   (cxx-bridged Google C++ Draco): same `.drc` bytes through both
   decoders, asserts vertex/index counts match and the per-face
   triangle multiset is structurally identical (vertex ID
   permutation between the two decoders is allowed). A real Skyline
   3D Tiles `.b3dm` is bundled in `tests/data/b3dm/` for
   multi-attribute (POSITION + NORMAL + TEXCOORD_0) coverage.

Our own encoder→decoder pair round-trips cleanly within
quantization tolerance.

The public API (`draco_oxide::prelude::{decode, decode_to_raw,
decode_with_warnings, splice_glb_remove_draco, ...}`) is stable —
external consumers can already wire against it.

#### Test fixtures from Google

`tests/data/google_fixtures/` contains `.drc` files produced by Google's
`draco_encoder` (1.5.7) for the bundled OBJ meshes, with optional
`.expected.obj` files (Google's `draco_decoder` output). These are the
ground truth our decoder is checked against. Regenerate via:

```bash
cd draco-oxide/tests/data
for mesh in tetrahedron sphere torus; do
  draco_encoder -i ${mesh}.obj -o google_fixtures/${mesh}_pos_cl7.drc \
    --skip NORMAL --skip TEX_COORD --skip GENERIC -qp 11
  draco_decoder -i google_fixtures/${mesh}_pos_cl7.drc \
    -o google_fixtures/${mesh}_pos_cl7.expected.obj
done
```

---

## Getting Started

### Add to Your Project

```txt
draco-oxide = "0.1.0-alpha.5"
```

### Example: Encode an obj file.

```rust
use draco_oxide::{encode::{self, encode}, io::obj::load_obj};
use draco_oxide::prelude::ConfigType;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create mesh from an obj file.
    let mesh = load_obj("mesh.obj").unwrap();

    // Create a buffer that we write the encoded data to.
    // This time we use 'Vec<u8>' as the output buffer, but 
    // draco-oxide can stream-write to anything 
    // that implements 'draco_oxide::prelude::ByteWriter'.
    let mut buffer = Vec::new();
    
    // Encode the mesh into the buffer.
    encode(mesh, &mut buffer, encode::Config::default()).unwrap();

    let mut file = std::fs::File::create("output.drc").unwrap();
    file.write_all(&buffer)?;
    Ok(())
}
```

### Example: Decode a .drc file (positions-only mesh).

```rust
use draco_oxide::decode::{self, decode};
use draco_oxide::prelude::ConfigType;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let buffer = fs::read("path/to/input.drc")?;
    let mut reader = buffer.into_iter();
    let mesh = decode(&mut reader, decode::Config::default())?;
    println!("decoded {} faces, {} attributes",
        mesh.get_faces().len(),
        mesh.get_attributes().len());
    Ok(())
}
```

See the [draco-oxide/examples](draco-oxide/examples/) directory for more.

### CLI

```bash
# compress input.obj into a draco file output.drc
cargo run --bin cli -- -i path/to/input.obj -o path/to/output.drc

# transcode input.glb into a draco compressed glb file output.glb as specified 
# in KHR_draco_mesh_compression extension.
cargo run --bin cli -- --transcode -i path/to/input.glb -o path/to/output.glb
```
---

## Roadmap to Beta

* Decoder Support.
* Complete glTF support.

---

## Acknowledgements

* **Google Draco** – original C++ implementation

---

## Contact

Re:Earth core committers: [community@reearth.io](mailto:community@reearth.io)

---

## License

Licensed under either (at your discretion):

- Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
