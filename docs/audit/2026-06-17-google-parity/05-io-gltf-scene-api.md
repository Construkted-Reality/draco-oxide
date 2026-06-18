# Parity audit 05 — I/O & file formats, glTF/scene transcoding, config/presets, public API

Date: 2026-06-17
Subject: `draco-oxide` (Rust port) vs Google Draco C++ reference (`google-draco-reference`, latest main).
Scope: `src/draco/io/`, `src/draco/scene/`, `src/draco/compression/config/`, `encode.h`/`decode.h`/`encode_base.h`/`expert_encode.h`.

All C++ paths are relative to `.../google-draco-reference/`. All Rust paths are relative to `.../draco-oxide/draco-oxide/`.

## Summary

Rough port coverage per area:

- **File I/O (OBJ/PLY/STL/point-cloud/file utils): ~8%.** Only OBJ *read* exists (`src/io/obj/mod.rs`, via `tobj`). No OBJ writer, no PLY, no STL, no point-cloud I/O, no file-reader/writer factory abstraction. C++ ships 6+ formats with readers and writers.
- **glTF + transcoder: ~30%.** A single-direction, GLB-only, hand-rolled-JSON Draco *compressor* exists (`src/io/gltf/transcoder.rs`) plus a Draco-*stripping* decoder (`src/io/gltf/draco_decoder.rs`). It handles a fixed attribute set on triangle primitives and passes everything else through verbatim. C++ `gltf_decoder.cc` (122 KB) and `gltf_encoder.cc` (145 KB) are full bidirectional Mesh⇄glTF and Scene⇄glTF transcoders.
- **Scene graph: ~25% (data model only, zero pipeline).** `src/core/scene/mod.rs` defines a faithful-looking scene graph (nodes, TRS, mesh groups, skins, lights, instance arrays, animations, libraries) but it is `pub(crate)`, never constructed by any I/O path, never encoded/decoded, and contains a stubbed transform composition. It is dead data structure, not wired to anything.
- **Compression config / presets: ~10%.** The Rust `Config` (`src/encode/mod.rs:25`) exposes only edgebreaker + per-attribute explicit quantization + per-attribute bit override. There is **no speed/compression-level (0–10), no encoding-method selection, no prediction-scheme selection, no feature flags, no `DracoCompressionOptions` preset surface** (color/normal/tangent/weight/generic default bits, grid quantization). The transcoder's `TranscoderConfig` is a thin wrapper that does not expose compression level or per-type quantization at all.
- **Public encode/decode API: ~20%.** `encode()`/`decode()` free functions exist and round-trip a mesh, but the C++ `Encoder`/`ExpertEncoder`/`Decoder`/`EncoderBase` class surface (speed options, expert per-attribute-id options, encoded-property tracking, skip-attribute-transform, point-cloud entry points, geometry-type query) is essentially absent.

Biggest gaps: no point-cloud path anywhere; no speed/method/prediction selection (core to the C++ encoder contract); scene graph is decorative; PLY/STL/OBJ-write entirely missing; transcoder is GLB-only and one-shot per direction.

## Coverage matrix

| Feature | C++ location | Rust location | Status | Notes |
|---|---|---|---|---|
| OBJ read | `io/obj_decoder.cc` | `src/io/obj/mod.rs:14` (`load_obj`) | Partial | Reads POSITION/NORMAL/TEXCOORD via `tobj`; materials/`.mtl`, object/group names, `use_metadata` dropped. `.expect()` panic on failure (`obj/mod.rs:21`). |
| OBJ write | `io/obj_encoder.cc` | — | Missing | No OBJ writer. |
| PLY read/write | `io/ply_decoder.cc`, `io/ply_encoder.cc`, `io/ply_reader.cc` | — | Missing | No PLY anywhere. |
| STL read/write | `io/stl_decoder.cc`, `io/stl_encoder.cc` | — | Missing | No STL anywhere. |
| Point-cloud I/O | `io/point_cloud_io.cc/.h` | — | Missing | No point-cloud read/write/encode/decode at all. |
| Mesh I/O dispatch by ext | `io/mesh_io.cc/.h` | — | Missing | No `ReadMeshFromFile`/`WriteMeshIntoStream` equivalent that chooses decoder by extension. |
| Scene I/O (read/write glTF scene) | `io/scene_io.cc/.h` | — | Missing | `ReadSceneFromFile`/`WriteSceneToFile` have no counterpart; scene struct never populated from a file. |
| File reader/writer factory + stdio | `io/file_reader_factory.cc`, `io/file_writer_factory.cc`, `io/stdio_file_*.cc` | — | Missing | No pluggable file abstraction; Rust uses `std::fs`/byte slices directly. |
| File/path utils | `io/file_utils.cc`, `io/file_writer_utils.cc` | — | Missing | No basedir/splitpath/relative-path helpers. |
| glTF decode (full, Draco→Mesh/Scene) | `io/gltf_decoder.cc` (122 KB) | `src/io/gltf/draco_decoder.rs:48` (`decode_glb`), `geometry_extractor.rs` | Partial/Divergent | Rust decodes Draco buffers inside a GLB and re-emits *vanilla* glTF (strips Draco); it does not build a `Scene`/`Mesh` object graph, nodes, materials, or animations. |
| glTF encode (Mesh/Scene→glTF, optional Draco) | `io/gltf_encoder.cc` (145 KB), `io/gltf_encoder.h` | `src/io/gltf/transcoder.rs:98` (`transcode`) | Partial/Divergent | One-direction passthrough compressor: takes uncompressed GLB, Draco-compresses triangle primitives, rewrites accessors/bufferViews. No `EncodeFile`/scene export/.gltf+.bin/`OutputType` parity. GLB in → GLB or glTF+bin out only. |
| KHR_draco_mesh_compression extension wiring | `io/gltf_encoder.cc`, `io/gltf_decoder.cc` | `src/io/gltf/draco_extension.rs`, `draco_decoder.rs` | Implemented (narrow) | Hand-rolled `serde_json` mutation of `extensions`/`accessors`/`bufferViews`. Works for the supported attribute set. |
| glTF attributes: POSITION/NORMAL/TEXCOORD_n/COLOR_n/TANGENT | `gltf_decoder.cc`/`gltf_encoder.cc` | `transcoder.rs:551–581` | Partial | Encoded by transcoder. COLOR VEC3→VEC4 (α=1) fallback `transcoder.rs:566`. |
| glTF JOINTS_n / WEIGHTS_n (skinning attrs) | `gltf_decoder.cc` | `transcoder.rs:592` (skipped) | Missing | Generic/skinning attributes skipped, not compressed. |
| glTF generic `_*` / app-specific attributes | `gltf_encoder.cc` (uses `kDracoMetadataGltfAttributeName`) | `transcoder.rs:583` (`_FEATURE_ID_*` only) | Partial/Divergent | Only `_FEATURE_ID_*` handled, truncated to u16 (`transcoder.rs:720`); other generics dropped. |
| glTF scene nodes / hierarchy / TRS | `gltf_decoder.cc`, `scene/scene_node.h` | data-only `src/core/scene/mod.rs:769` | Missing (in pipeline) | Transcoder never reads `nodes`/`scenes`; node transforms not consumed. Scene struct exists but unused by I/O. |
| EXT_mesh_gpu_instancing / instance arrays | `scene/instance_array.cc`, `gltf_decoder.cc` | data-only `src/core/scene/mod.rs:538` | Missing (in pipeline) | Struct present; never parsed or emitted. |
| KHR_lights_punctual | `scene/light.cc`, `gltf_decoder.cc` | data-only `src/core/scene/mod.rs:403` | Missing (in pipeline) | Struct present; never parsed or emitted. |
| Materials / textures / images / KTX2 | `io/texture_io.cc`, `gltf_*` | passthrough only in `transcoder.rs` | Partial | Bytes preserved/remapped, not modeled (`MaterialLibrary`/`TextureLibrary` in scene struct are unused). |
| Animations / skins / morph targets | `gltf_decoder.cc`, `scene/scene.h` | passthrough only | Missing (modeling) | Preserved as opaque bufferViews; not reconstructed or re-indexed semantically. |
| `Encoder` basic API (encode mesh/point cloud) | `compression/encode.h` | `src/encode/mod.rs:147` (`encode`) | Partial | Free `encode(mesh, writer, cfg)`; mesh only, no point cloud. |
| `Decoder` API + geometry-type query | `compression/decode.h` | `src/decode/mod.rs:15` (`decode`) | Partial | No `GetEncodedGeometryType`, no point-cloud decode, no `DecodeBufferToGeometry(into)`. |
| `Decoder::SetSkipAttributeTransform` | `decode.h:68` | — | Missing | No skip-transform / leave-quantized path. |
| `ExpertEncoder` (per-attribute-id options) | `compression/expert_encode.h` | — | Missing | No expert encoder; options are per **type**, not per attribute id. |
| `SetSpeedOptions(enc,dec)` 0–10 | `encode.h:70`, `encode_base.h:56` | — | Missing | No speed concept; encoder always runs one path. |
| `SetEncodingMethod` (SEQUENTIAL vs EDGEBREAKER) | `encode.h:130`, `compression_shared.h:55` | `Config.encoder_method` exists (`encode/mod.rs:34`) but no setter; sequential decode unimplemented | Divergent | Field defaults to Edgebreaker, not user-settable; `EncoderMethod::Sequential` decode returns error (`decode/connectivity/mod.rs:58`). |
| `SetEncodingSubmethod` (EB standard/valence) | `encode_base.h:64`, `compression_shared.h:122` | — | Missing | No submethod selection. |
| `SetAttributePredictionScheme` | `encode.h:109`, `expert_encode.h:132` | — | Missing | Prediction scheme is auto/fixed; no user override or validation (`CheckPredictionScheme`). |
| `SetAttributeQuantization(type,bits)` | `encode.h:76` | `Config::set_attribute_quantization_bits` (`encode/mod.rs:123`) | Implemented | Per-type bit override mirrors C++. |
| `SetAttributeExplicitQuantization` | `encode.h:83` | `Config::set_attribute_explicit_quantization` (`encode/mod.rs:85`) | Implemented | Matches C++ origin/range/bits semantics. |
| `SetUseBuiltInAttributeCompression` | `expert_encode.h:81` | — | Missing | No toggle for built-in entropy coding. |
| `SetAttributeGridQuantization` (spacing) | `expert_encode.h:138` | — | Missing | No grid quantization. |
| `SetTrackEncodedProperties` / encoded counts | `encode_base.h:44` | — | Missing | No num_encoded_points/faces tracking. |
| `DracoCompressionOptions` presets (level 0–10, per-type default bits, grid) | `compression/draco_compression_options.h` | — | Missing | No preset struct; transcoder `TranscoderConfig` (`transcoder.rs:45`) exposes neither compression level nor per-type bits. |
| Decoder/Encoder feature flags | `config/encoder_options.h`, `encoding_features.h` | — | Missing | No supported-feature negotiation. |

## Correctness findings

1. **[HIGH] Encoding method is not selectable and Sequential is non-functional.**
   C++ `Encoder::SetEncodingMethod` (`compression/encode.h:130`) accepts `MESH_SEQUENTIAL_ENCODING` or `MESH_EDGEBREAKER_ENCODING` (`compression/config/compression_shared.h:55`). Rust `Config` stores `encoder_method` (`src/encode/mod.rs:34`) but exposes **no setter**, defaults to Edgebreaker (`src/encode/mod.rs:55`), and the decoder explicitly rejects sequential: `EncoderMethod::Sequential => Err(Err::SequentialNotImplemented)` (`src/decode/connectivity/mod.rs:58`). Why it matters: any bitstream a third party produced with sequential encoding cannot be decoded, and the documented method-selection contract is unmet.

2. **[HIGH] No speed / compression-level surface; presets silently absent.**
   C++ encoders are defined by speed (`encode.h:70`, default 5 via `encoder_options.h:46`) and the transcoder by `compression_level` 0–10 (`draco_compression_options.h:74`), which select methods/prediction/entropy variants. Rust `Config` (`src/encode/mod.rs:25`) and `TranscoderConfig` (`src/io/gltf/transcoder.rs:45`) have no level/speed field at all — the encoder always runs one fixed path. Why it matters: callers porting from C++ that set a compression level get a different, fixed trade-off with no error, and cross-implementation reproducibility of "level N" output is impossible.

3. **[HIGH] `DracoCompressionOptions` per-type default quantization is not honored by the transcoder.**
   C++ transcoding uses per-type defaults: position 11, normal 8, tex 10, color 8, generic 8, tangent 8, weight 8 (`draco_compression_options.h:75–81`). The Rust transcoder hands the geometry to `encode::encode` with `self.config.draco.clone()` (`src/io/gltf/transcoder.rs:497`) and `TranscoderConfig::default` carries a plain `DracoConfig::default()` (`transcoder.rs:53`) with empty per-type maps — so quantization falls back to the encoder's internal defaults (Position 11 / TexCoord 10 / Normal 8 per `encode/mod.rs:116` doc) and color/tangent/weight/generic have **no preset path**. Why it matters: a transcode produces different precision than C++ `draco_transcoder` for color/tangent/weight, and there is no API to set them per the C++ contract.

4. **[MEDIUM] glTF transcoder is single-direction and GLB-bound, diverging from `GltfEncoder`/`GltfDecoder`.**
   C++ `GltfDecoder` builds a `Mesh`/`Scene` from glTF (incl. Draco), and `GltfEncoder` writes `Mesh`/`Scene` to `.gltf`+`.bin` or `.glb` with `OutputType` COMPACT/VERBOSE (`io/gltf_encoder.h:44`). Rust `transcoder.rs` only ingests an *uncompressed* GLB and emits a Draco-compressed GLB/glTF (`transcoder.rs:98`); `draco_decoder.rs:48` only goes the other way (Draco-GLB → vanilla GLB). Neither produces a `Scene`/`Mesh` object. Multi-buffer / external-URI glTF is rejected (`transcoder.rs` single embedded buffer assumption). Why it matters: cannot load a normal `.gltf`+`.bin` asset, cannot export geometry to glTF from a `Mesh`, no scene round-trip.

5. **[MEDIUM] Scene graph data model is present but inert, and its transform composition is a stub.**
   `src/core/scene/mod.rs` defines nodes with parent/child indices and per-node TRS (`mod.rs:769–888`), mesh groups, skins, lights, instance arrays, animations, and material/texture/structural-metadata libraries — mirroring C++ `scene/scene.h`. But the struct is `pub(crate)` (constructor `mod.rs:693`), is never built by any I/O path, and is never encoded/decoded. `compute_transformation_matrix` (`mod.rs:270`) composes scale and translation but **omits the rotation multiply** (comment at `mod.rs:287`: "In a real implementation, you'd multiply result * rotation_matrix"). Why it matters: anything relying on node world transforms would get wrong matrices; more fundamentally the scene path is non-functional, so C++ `scene_io`/`scene_utils` capabilities (instancing, hierarchy flatten, dedup) are entirely unported.

6. **[MEDIUM] No point-cloud path on either side of the API.**
   C++ exposes `EncodePointCloudToBuffer`/`DecodePointCloudFromBuffer` and `GetEncodedGeometryType` (`encode.h:44`, `decode.h:36,43`). Rust `EncodedGeometryType::PointCloud` exists in the header enum but is `#[allow(unused)]` (`src/encode/header/mod.rs:10`); `encode()`/`decode()` are mesh-only with no geometry-type detection branch. Why it matters: KD-tree point-cloud compression — a whole Draco subsystem — is absent and the public API cannot represent it.

7. **[MEDIUM] glTF skinning/generic attributes are dropped or lossily handled.**
   C++ compresses JOINTS_n/WEIGHTS_n and app-specific generic attributes (named via `kDracoMetadataGltfAttributeName`, `gltf_encoder.h:94`). Rust `transcoder.rs:592` skips unrecognized attributes, and only `_FEATURE_ID_*` generics are handled — truncated from f32 to u16 (`transcoder.rs:720–722`). Why it matters: skinned meshes lose joint/weight data through the transcoder; generic attribute values can be silently corrupted by the u16 truncation.

8. **[LOW] OBJ reader panics instead of returning an error, and ignores materials/metadata.**
   `src/io/obj/mod.rs:21` uses `.expect("Failed to load OBJ file")`, vs C++ `obj_decoder.cc` returning a `Status`. Materials (`_materials` discarded), object/group names, and the `use_metadata` option (`mesh_io.h:90`) are unsupported. Why it matters: a malformed OBJ aborts the process; material/metadata-driven workflows are unsupported.

9. **[LOW] Decoder has no `SetSkipAttributeTransform` / per-attribute decode options.**
   C++ `Decoder::SetSkipAttributeTransform` (`decode.h:68`) lets callers receive quantized values plus the transform descriptor. Rust `decode()` (`src/decode/mod.rs:15`) ignores its `cfg` for this purpose (`decode_with_warnings` takes `_cfg`, `mod.rs:34`) and always dequantizes. Why it matters: consumers needing raw quantized attributes (re-quantization pipelines) cannot get them.

## Missing features

### File I/O
- **PLY read/write** (`io/ply_*`) — blocks ingesting/exporting PLY meshes & point clouds.
- **STL read/write** (`io/stl_*`) — blocks STL workflows.
- **OBJ write + `.mtl`** (`io/obj_encoder.cc`) — blocks exporting to OBJ.
- **Point-cloud I/O** (`io/point_cloud_io.*`) — blocks all point-cloud read/write.
- **Mesh-I/O extension dispatch** (`io/mesh_io.*`) and **file reader/writer factories + path utils** (`io/file_*`) — blocks a uniform "load by filename" entry and pluggable backends.

### glTF / transcoder
- **Full glTF→Mesh/Scene decode** (`gltf_decoder.cc`) — blocks loading standard (non-GLB / multi-buffer / external-URI) assets and building an in-memory scene.
- **Mesh/Scene→glTF encode** (`gltf_encoder.cc`, `EncodeFile`, `.gltf`+`.bin`, COMPACT/VERBOSE) — blocks exporting geometry to glTF from Draco data.
- **Scene-level transcode** (nodes, hierarchy, TRS, instancing, lights, materials/textures modeled, animations/skins re-indexed) — blocks anything beyond flat per-primitive geometry passthrough.
- **JOINTS/WEIGHTS + general generic-attribute compression** — blocks skinned-mesh transcode and lossless generic attributes (current `_FEATURE_ID` u16 truncation is lossy).

### Scene
- **Wiring the scene data model to any pipeline** — `core/scene/mod.rs` exists but is `pub(crate)` and never populated/encoded/decoded; blocks `scene_io` and `scene_utils` parity (instance dedup, hierarchy flattening, `SceneToDracoTranscoder`).
- **Correct TRS composition** (rotation multiply, `mod.rs:287`) — blocks correct world-transform computation.

### Config / presets
- **Speed options 0–10** (`encode_base.h:56`) and **`compression_level`** (`draco_compression_options.h:74`) — blocks method/quality selection and C++-equivalent "level N" output.
- **Encoding method + submethod selection** (`encode.h:130`, `encode_base.h:64`) and a **working sequential path** — blocks sequential-encoded streams (encode & decode).
- **Per-attribute prediction-scheme selection + validation** (`encode.h:109`, `encode_base.h:68`) — blocks forcing/validating predictors.
- **`DracoCompressionOptions` per-type default bits (color/tangent/weight/generic) + grid quantization** (`draco_compression_options.h`, `expert_encode.h:138`) — blocks preset-parity transcodes and grid-snapped positions.
- **Feature flags / built-in-compression toggle** (`encoder_options.h`, `expert_encode.h:81`) — blocks decoder-feature negotiation and third-party-entropy workflows.

### Public API
- **`ExpertEncoder`** (per-attribute-id options) — blocks per-attribute (not per-type) control.
- **Point-cloud encode/decode entry points + `GetEncodedGeometryType`** (`encode.h:44`, `decode.h:36`) — blocks point-cloud use and type probing.
- **`Decoder::SetSkipAttributeTransform`** (`decode.h:68`) — blocks receiving quantized-but-untransformed attributes.
- **`SetTrackEncodedProperties` / encoded counts** (`encode_base.h:44`) — blocks reporting encoded point/face counts.
