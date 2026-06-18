# Parity audit 04 — Point cloud, sequential encoding, metadata, animation

Date: 2026-06-17
Reviewer: code-parity pass (review only, no fixes)

- Rust port: `draco-oxide/draco-oxide/src`
- C++ reference: `google-draco-reference/src/draco`

## Summary

This subsystem group is almost entirely absent in the Rust port, as expected.
**Point cloud compression: ~0% implemented** — the C++ KD-tree and sequential
point-cloud encoders/decoders (~1.7k LoC of encoder/decoder + ~2.4k LoC of
algorithms) have no Rust counterpart at all; `core/point_cloud/` and
`core/point_cloud_builder/` are empty 1-byte `mod.rs` stubs, and the
`PointCloud` data structure does not exist. **Animation: 0% implemented** —
no file in the Rust tree references animation, keyframes, or skins. **Metadata:
~10% implemented and divergent** — a decode-side reader and an empty encode-side
stub exist, but they use a hand-rolled wire format that does not match C++'s
varint-based `MetadataEncoder`, and structural-metadata / property-table
(EXT_structural_metadata) is wholly absent. **Sequential connectivity: present
but dead** — a `Sequential` direct-index connectivity encoder exists yet is
never wired in (the attribute encoder `unimplemented!()`s on it and the decoder
returns `SequentialNotImplemented`), so in practice only Edgebreaker mesh
encoding works.

## Coverage matrix

| Feature | C++ location | Rust location | Status | Notes |
|---|---|---|---|---|
| `PointCloud` data structure | `point_cloud/point_cloud.{h,cc}` | `core/point_cloud/mod.rs` | **Missing** | Rust file is a 1-byte empty stub; no `PointCloud` type exists. Mesh is the only geometry type. |
| `PointCloudBuilder` | `point_cloud/point_cloud_builder.{h,cc}` | `core/point_cloud_builder/mod.rs` | **Missing** | 1-byte empty stub. |
| Base `PointCloudEncoder`/`Decoder` | `compression/point_cloud/point_cloud_{encoder,decoder}.{h,cc}` | — | **Missing** | No base PC encode/decode pipeline. |
| KD-tree PC encoder/decoder | `compression/point_cloud/point_cloud_kd_tree_{encoder,decoder}.{h,cc}` | — | **Missing** | `KDTREE` method (`point_cloud_compression_method.h`) unimplemented. |
| Integer KD-tree algorithm | `algorithms/integer_points_kd_tree_{encoder,decoder}.{h,cc}` | — | **Missing** | Core lossless PC compression algorithm absent. |
| Dynamic integer KD-tree algorithm | `algorithms/dynamic_integer_points_kd_tree_{encoder,decoder}.{h,cc}` | — | **Missing** | — |
| Float points tree (quantize+encode) | `algorithms/float_points_tree_{encoder,decoder}.{h,cc}`, `quantize_points_3.h` | — | **Missing** | — |
| Sequential PC encoder/decoder | `compression/point_cloud/point_cloud_sequential_{encoder,decoder}.{h,cc}` | — | **Missing** | The C++ "sequential" *point cloud* path (used also by animation) has no Rust analog. |
| Sequential **mesh connectivity** encoder | (mesh side) `compression/mesh/mesh_sequential_encoder.*` | `encode/connectivity/sequential.rs`, `shared/connectivity/sequential.rs` | **Partial / dead** | Direct-index writer exists (8/16/21-varint/32-bit). Never invoked: `encode/attribute/attribute_encoder.rs:280` `unimplemented!()`; decode `decode/connectivity/mod.rs:58` returns `SequentialNotImplemented`. |
| Metadata encode | `metadata/metadata_encoder.{h,cc}` | `encode/metadata/mod.rs` | **Stub** | `encode_metadata` writes a literal `u32 0` and returns; ignores the mesh. |
| Metadata decode | `metadata/metadata_decoder.{h,cc}` | `decode/metadata/mod.rs` | **Partial / divergent** | Reads key/value/sub-metadata, but uses `u8` length prefixes and does not follow C++'s `EncodeVarint`+`EncodeString` framing (see findings). Result is never surfaced to the Mesh API. |
| Metadata entry value types (int/double/string/binary + arrays) | `metadata/metadata.{h,cc}` (`EntryValue`, `AddEntryInt/Double/String/Binary/...Array`) | — | **Missing** | Rust stores raw `Vec<u8>` key/value only; no typed accessors. |
| Geometry / attribute metadata | `metadata/geometry_metadata.{h,cc}` | — | **Missing** | No per-attribute metadata, no `att_unique_id` mapping in the C++ sense. |
| Structural metadata + schema | `metadata/structural_metadata*.{h,cc}` | — | **Missing** | EXT_structural_metadata (~2.2k LoC). Entirely absent. |
| Property table / property attribute | `metadata/property_table.{h,cc}`, `metadata/property_attribute.{h,cc}` | — | **Missing** | — |
| Keyframe animation encode/decode | `animation/keyframe_animation_{encoder,decoder}.{h,cc}` | — | **Missing** | No Rust file references animation. |
| Animation / NodeAnimationData | `animation/animation.{h,cc}`, `animation/node_animation_data.h` | — | **Missing** | — |
| Skin | `animation/skin.{h,cc}` | — | **Missing** | — |

## Correctness findings

1. **Metadata wire format diverges from C++ (misleading "present" decoder).**
   `decode/metadata/mod.rs` reads each key and value as a single `u8`
   length prefix followed by raw bytes (`read_from`, lines 32–41/59–68), and
   reads the sub-metadata count via `leb128`. C++ `MetadataEncoder::EncodeMetadata`
   (`metadata/metadata_encoder.cc:21-48`) instead: encodes `num_entries` as a
   varint, each key via `EncodeString` (varint length prefix), each value's
   `data_size` as a varint, then a varint sub-metadata count. The Rust reader
   would mis-parse any real Draco metadata block. Because `encode_metadata`
   currently emits only a `u32 0` and the encoder sets `metdata: false` by
   default, the two sides are internally self-consistent on empty metadata only;
   the decoder cannot read genuine upstream Draco metadata. Flagged as
   **divergent**, not implemented.

2. **Decoded metadata is dead.** `decode_metadata` returns a `Metadata` struct
   that both call sites bind to `_metadata` (`decode/mod.rs:44`, `:237`) and
   discard. The struct is `#[allow(dead_code)]` and never reaches the public
   API. Functionally inert.

3. **`Sequential` connectivity encoder is present but unreachable.**
   `encode/connectivity/sequential.rs` is a complete-looking direct-index
   encoder, but the attribute-encoding path hard-fails on the sequential output
   (`encode/attribute/attribute_encoder.rs:279-280` `unimplemented!(...)`), and
   the decoder explicitly rejects it (`decode/connectivity/mod.rs:58`). Reading
   this file in isolation could mislead a reviewer into thinking sequential
   encoding works end-to-end. It does not.

4. **`EncodedGeometryType::PointCloud` exists but is never producible.** The
   header enum carries a `PointCloud` variant (`encode/header/mod.rs:10,17`),
   but `Config::default()` hardcodes `TrianglarMesh` (`encode/mod.rs:54`) and
   there is no API to construct or encode a point cloud. The variant is
   aspirational scaffolding.

## Missing features

### Point cloud (blocks all point-cloud support)
- `PointCloud` and `PointCloudBuilder` data structures — without these there is
  no in-memory point-cloud representation to encode/decode at all.
- Base `PointCloudEncoder`/`PointCloudDecoder` pipeline — the dispatch layer
  that mesh encoding's analog already has.
- KD-tree compression (`KDTREE` method): integer, dynamic-integer, and
  float-points tree encoders/decoders, plus `quantize_points_3`. This is the
  primary lossless/quantized point-cloud codec; its absence means draco-oxide
  cannot read or write any KD-tree-encoded `.drc` point cloud.
- Sequential point-cloud encoder/decoder — needed both for the sequential PC
  method and as the base class that keyframe animation reuses.

### Metadata (blocks metadata round-trip and glTF feature interop)
- Typed `EntryValue` model and `AddEntry*`/`GetEntry*` accessors
  (int/double/string/binary + array variants) — currently only opaque
  `Vec<u8>` blobs.
- Varint-correct encode/decode matching C++ framing — current decoder cannot
  parse real Draco metadata; encoder emits nothing.
- Geometry/attribute metadata with `att_unique_id` association — blocks
  preserving per-attribute names/metadata through a round-trip.
- Structural metadata + property table/attribute (EXT_structural_metadata) —
  blocks glTF structural-metadata interop. This is the single largest absent
  chunk (~2.2k C++ LoC).

### Animation (blocks all animation support)
- Keyframe animation encode/decode, `Animation`/`NodeAnimationData`, and `Skin`
  — none present. Blocks any glTF animation/skinning round-trip. Note C++
  keyframe animation is implemented as a thin wrapper over the sequential
  point-cloud encoder, so it is also transitively blocked by the missing
  point-cloud pipeline.

### Sequential mesh connectivity (blocks the non-Edgebreaker mesh path)
- Wiring the existing `Sequential` connectivity encoder into the attribute
  encoder and a matching decoder. The encoder writer exists; the integration
  and the decode side do not.
