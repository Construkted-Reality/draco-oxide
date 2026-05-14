# draco-oxide

&#x20;&#x20;

[![Crates.io](https://img.shields.io/crates/v/draco-oxide)](https://crates.io/crates/draco-oxide)
[![Documentation](https://docs.rs/draco-oxide/badge.svg)](https://docs.rs/draco-oxide) 

`draco-oxide` is a high-performance Rust re-write of Google’s [Draco](https://github.com/google/draco) 3D-mesh compression library, featuring efficient streaming I/O and seamless WebAssembly integration.

> **Status:** **Alpha** – Encoder + decoder are both functional for positions, normals, and UVs. UV precision on heavily-seamed meshes is degraded (the decoder operates on the universal corner table; encoder uses per-attribute corner tables for seam-heavy attributes — wiring that through is a follow-up).

---

## Features

| Component              | Alpha  | Beta Roadmap       |
| ------------------     | -----  | ------------------ |
| Mesh Encoder           | ✅     | Performance optimization |
| Mesh Decoder           | ✅     | Per-attribute corner table for UV/normal seams |
| glb Transcoder (basic)| ✅     | Animation and many more extensions  |
| glb Decoder (basic)   | ✅     | Per-attribute corner table for UV/normal seams |

### Encoder Highlights

* Triangle‑mesh compression with configurable speed/ratio presets.
* Basic glb transcoder (`*.glb` → `*.glb` with mesh buffer compressed via [KHR_draco_mesh_compression extension](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_draco_mesh_compression)).
* Pure‑Rust implementation.
* `no_std` + `alloc` compatible; builds to **WASM32**, **x86\_64**, **aarch64**, and more.

### Decoder Status

The decoder pipeline is end-to-end working for positions, normals,
UVs, and vertex colors on both this repo's encoder output and
Google's reference encoder output. UV decode uses a fallback path
that gives preview-grade fidelity on UV-seamed meshes (~7.5e-2 max
L2); the per-attribute corner table infrastructure is wired but
the full pixel-perfect path needs Traverser visit-order
reconciliation between encoder and decoder. What's wired up today:

* Header decode + metadata flag handling.
* Edgebreaker connectivity decode for both **Standard** and **Valence**
  traversal modes (C/R/L/E/S symbols, topology splits for higher-genus
  meshes, start-face-config replay via RabsDecoder, per-context symbol
  arrays for Valence with on-the-fly active-context computation from
  vertex valences).
* Per-attribute decode pipeline: prediction-scheme dispatch,
  WrappedDifference / Difference / NoTransform / OctahedralOrthogonal
  inverse transforms, and QuantizationCoordinateWise / ToBits /
  OctahedralQuantization deportabilization (positions, UVs, normals).
* `MeshParallelogramPrediction` inverse for positions.
* `MeshNormalPrediction` inverse for normals (face-normal sum + flip
  bits via RABS), with normal output at the 8-bit oct quantization
  theoretical floor (~5e-2 max L2 error vs Google reference encoder).
* `MeshPredictionForTextureCoordinates` inverse for UVs (3D triangle
  plane projection with two-orientation sign-flip + reverse-RLE
  orientation bits via RABS).
* Vertex colors (`AttributeType::Color`, N=3 RGB or N=4 RGBA) via
  `QuantizationCoordinateWise` + `WrappedDifference`.
* Both `LengthCoded` and `DirectCoded` symbol entropy paths (matching
  Google's `DecodeTaggedSymbols` / `DecodeRawSymbols` formats — see
  `compression/entropy/symbol_decoding.cc`).
* `Mesh` reconstruction from decoded faces + attributes.
* glTF `KHR_draco_mesh_compression` extraction wrapper
  (`io::gltf::draco_decoder::decode_glb`).
* CLI `--decode` mode: `cargo run -p cli -- --decode -i input.drc -o output.obj`.

**Verified compatible with Google's reference encoder** for positions-
only meshes at default settings (`-cl 7 -qp 11`). See
`tests/google_compat.rs`:

| Fixture     | Mesh             | Verts | Faces | Edgebreaker | Max L_inf error vs Google ref |
| ----------- | ---------------- | ----: | ----: | ----------- | -----------------------------: |
| tetrahedron | closed manifold  |     4 |     4 | Standard    |                            0.0 |
| sphere      | closed manifold  |   114 |   224 | Standard    |                          1e-6 |
| torus       | genus-1 manifold | 2 051 | 4 095 | Valence     |                          1e-6 |
| bunny       | partial mesh     | ~35K  |  ~69K | Valence     |                          1e-6 |

Our own encoder→decoder pair round-trips cleanly for positions-only
tetrahedron, sphere, torus, and bunny within quantization tolerance.

Known gaps before "production ready":

* Per-attribute corner tables — the encoder uses
  `attribute_corner_table` for normal/UV-seamed meshes; the decoder
  always uses the universal corner table. Visual fidelity is
  acceptable for preview but not pixel-perfect on heavily-seamed
  inputs (UV decode lands at ~7.5e-2 max L2 vs ~1e-3 quantization
  floor on tetrahedron).

The public API (`draco_oxide::decode::decode`) is stable — external
consumers can already wire against it.

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
draco-oxide = "0.1.0-alpha.9"
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
