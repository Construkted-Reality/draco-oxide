# Google Parity Audit — Connectivity Compression & Mesh Topology

Date: 2026-06-17
Subject (Rust): `draco-oxide/src/{encode,decode}/connectivity/`, `core/corner_table/`, `core/mesh/`, `shared/connectivity/`
Reference (C++): `src/draco/compression/mesh/`, `src/draco/mesh/`, corner-table parts of `core/`
Reference bitstream version targeted by Rust: **2.2** (header writes `2,2` — `encode/header/mod.rs:35-36`).

> NOTE: I could not execute the test suite in this environment. The C++ dev-dependency
> `draco_decoder = "0.0.26"` (a workspace dev-dep) fails to link (`-ldraco` not found),
> which blocks **all** test binaries including `round_trip` and `google_compat`. The
> "bit-perfect vs Google" README claim is therefore **unverified empirically here**; the
> findings below are from code reading only. Restoring `libdraco` so `cargo test
> --test google_compat` runs is the single highest-value follow-up.

---

## Summary

Roughly **60–70%** of the connectivity/mesh subsystem is ported with fidelity for the
common case (closed, manifold, single- or multi-component meshes, Standard Edgebreaker
traversal, positions + seamed attributes). The **Standard traversal encoder and decoder
are faithful and symmetric**, including topology-split (handle/hole) handling, the CrLight
symbol code (C=1 bit / S,L,R,E=3 bits, bit patterns `0x0/0x1/0x3/0x5/0x7`), start-face
rABS, attribute-seam rABS, and the Spirale-Reversi corner-table rebuild. The biggest gaps
are: **(1) the encoder never runs mesh cleanup** (degenerate-face / duplicate-face /
unused-attribute removal) and the corner table does not subtract degenerate faces or
isolated vertices from the counts it writes — so any mesh with degenerate or duplicate
faces, or isolated vertices, produces a divergent (and likely Google-undecodable)
bitstream or panics; **(2) the Valence traversal path is half-wired** — the encoder's
`ValenceTraversal::encode` drops start-face configs and attribute seams entirely
(commented out), so it cannot be the default and is not byte-compatible; **(3) sequential
connectivity is divergent** (raw `u64` face count instead of varint, no `num_points`
field, no compressed-indices path). The biggest correctness risks are the degenerate/
isolated-count mismatch (high) and the unimplemented `use_single_connectivity`
(`unimplemented!()` panic) which Google selects automatically at speed ≥ 6.

---

## Coverage matrix

| Feature | C++ location | Rust location | Status | Notes |
|---|---|---|---|---|
| Edgebreaker top-level connectivity encode | `mesh_edgebreaker_encoder_impl.cc:269` | `encode/connectivity/edgebreaker.rs:499` | Implemented | Header field order matches (num_verts, num_faces, num_attr_data, num_symbols, num_split_symbols, split data, traversal buffer). |
| Standard traversal encoder (CRLES symbols) | `mesh_edgebreaker_traversal_encoder.h` | `encode/.../edgebreaker.rs:610` `DefaultTraversal` + `symbol_encoder.rs` | Implemented | Symbols reversed, C=1bit others=3bit, patterns `0x1/0x3/0x5/0x7` match. |
| Valence traversal encoder | `mesh_edgebreaker_traversal_valence_encoder.h` | `encode/.../edgebreaker.rs:739` `ValenceTraversal` | Partial/Divergent | Valence math + 6 contexts present, but `encode()` (`:881`) **omits start faces & attribute seams** (lines 890-891 commented out). Not selectable as default. See finding #4. |
| Predictive traversal encoder | `mesh_edgebreaker_traversal_predictive_encoder.h` | — | Missing | `EdgebreakerKind::Predictive` → `unimplemented!()` (`encode/connectivity/mod.rs:59`). Google also doesn't auto-select it; low impact. |
| Encoder method auto-selection (speed/size) | `mesh_edgebreaker_encoder.cc:25` | hardcoded | Divergent | Rust always writes `EdgebreakerKind::Standard` (`edgebreaker.rs:509`), ignores `cfg.traversal`. Google picks Valence for non-tiny meshes at speed<5. See finding #2. |
| `use_single_connectivity` (split-on-seams, speed≥6 default) | `mesh_edgebreaker_encoder_impl.cc:52`, `CreateCornerTableFromAllAttributes` | `edgebreaker.rs:133` | Missing | `unimplemented!("Single connectivity is not supported yet.")` — panics. See finding #6. |
| Start-face configuration (interior/boundary) | `..._traversal_encoder.h:61` + `rans_bit_encoder.cc:72` | `edgebreaker.rs:671-687` | Implemented | Both use static `zero_prob` rABS (Google's `RAnsBitEncoder` is NOT adaptive — it buffers, computes one `zero_prob`, writes `prob,size,bytes`). Rounding/clamp matches. |
| Attribute seam encoding (per-attr rABS) | `..._traversal_encoder.h:78`, `EncodeAttributeConnectivitiesOnFace` | `edgebreaker.rs:689-733` | Implemented | Iterates `processed_connectivity_corners.rev()`, `[c,next,prev]`, skips boundary + already-visited-opposite. Matches C++ dedup. |
| Topology split / handle / hole events | `EncodeSplitData` `:438`, `FindHoles` `:706`, `EncodeHole` `:620` | `encode_topology_splits` `:399`, `compute_boundaries` `:201`, `process_boundary` `:232` | Implemented | Delta+varint source/split ids, then 1-bit edge per event. Matches v2.2 (1 bit, was 2 in <2.2). |
| Start-face init-face ordering (reverse + append) | `mesh_edgebreaker_encoder_impl.cc:401-409` | `edgebreaker.rs:575-577` | Implemented | `init_face_connectivity_corners` reversed then appended. |
| Hole/handle decode (DecodeHoleAndTopologySplitEvents) | `mesh_edgebreaker_decoder_impl.cc:976` | `decode/.../edgebreaker.rs:323` `read_topology_splits` | Implemented | No hole-event read (correct for v2.1+). |
| Standard traversal decoder (Spirale Reversi) | `mesh_edgebreaker_decoder_impl.cc:535` | `decode/.../edgebreaker.rs:409` `replay_symbols` | Implemented | C/S/L/R/E handlers + topology-split active-corner resolution mirror C++ closely. |
| Valence traversal decoder | `mesh_edgebreaker_traversal_valence_decoder.h` | `decode/.../edgebreaker.rs:94,481,664` | Partial | Decode path exists (6 contexts, first symbol forced E, back-to-front consume). But the matching **encoder is incomplete (#4)**, so this is untested end-to-end and cannot round-trip with the Rust encoder. |
| Predictive traversal decoder | `mesh_edgebreaker_traversal_predictive_decoder.h` (legacy) | — | Missing | `EdgebreakerKind::Predictive` → `UnsupportedTraversal` (`decode/.../edgebreaker.rs:119`). Legacy-only in Google; low impact. |
| Attribute connectivity decode + corner-table build | `DecodeAttributeConnectivitiesOnFace` `:1130`, `:497` | `build_attribute_corner_tables` `:156`, `decode/.../attribute_corner_table.rs` | Implemented (Partial) | Present; but seam-bit-count replay relies on `primary_offsets` heuristic per symbol; correctness for multi-component + start-faces not verifiable here. |
| CornerTable construction (opposite corners) | `corner_table.cc:83` `ComputeOppositeCorners` | `core/corner_table/mod.rs:258` `compute_table` | Implemented | Same sink-vertex half-edge matching + mirrored-face guard (`mod.rs:317`). |
| Non-manifold EDGE breaking | `corner_table.cc:212` `BreakNonManifoldEdges` | `core/corner_table/mod.rs:153` `handle_no_manifold_edges` | Implemented | Direct port. |
| Non-manifold VERTEX splitting | `corner_table.cc:319` `ComputeVertexCorners` | `core/corner_table/mod.rs:351` `compute_left_most_corners` | Implemented | Creates new vertex + records parent. |
| Degenerate-face count / skip | `corner_table.cc:146,401` `IsDegenerated`/`NumDegeneratedFaces` | — | **Missing** | Rust counts degenerate corners as faces; never subtracts. See finding #1. |
| Isolated-vertex count | `corner_table.cc:392` `NumIsolatedVertices` | — | **Missing** | Encoder writes raw `num_vertices()` (`edgebreaker.rs:514`). See finding #1. |
| Unused-vertex handling | (cleanup removes them) | `core/corner_table/mod.rs:104-110` | Divergent | **Panics** on any unused vertex instead of cleaning. See finding #5. |
| `Valence()` semantics (count faces around vertex) | `corner_table.cc:415` | `core/corner_table/mod.rs:431` `vertex_valence` | Implemented | Swing-right count; matches face-count semantics. |
| MeshAttributeCornerTable (seam detect / recompute) | `mesh_attribute_corner_table.cc:41,129` | `core/corner_table/attribute_corner_table.rs` | Implemented | `InitFromAttribute`, `IsCornerOppositeToSeamEdge`, `RecomputeVertices` ported; has unit tests for seam/no-seam. |
| Mesh cleanup (degenerate/dup/unused removal) | `mesh_cleanup.cc` | — | **Missing** | No equivalent anywhere in encode path. See finding #1/#5. |
| Sequential connectivity encode | `mesh_sequential_encoder.cc:28` | `encode/connectivity/sequential.rs:76` | Divergent | Raw `u64` face count (not varint), no `num_points`, no compressed path. See finding #3. |
| Sequential connectivity decode | `mesh_sequential_decoder.cc` | — | Missing | `decode/connectivity/mod.rs:60` → `SequentialNotImplemented`. |
| Mesh stripifier | `mesh_stripifier.{cc,h}` | — | Missing | Not used by edgebreaker path. |
| Mesh splitter / connected components / features | `mesh_splitter.cc`, `mesh_connected_components.h`, `mesh_features.*` | — | Missing | Not on the core compression path; low impact. |

---

## Correctness findings

### 1. (HIGH) Degenerate faces and isolated vertices are not removed from bitstream counts
**Rust:** `encode/connectivity/edgebreaker.rs:514-515` writes
`leb128_write(self.corner_table.num_vertices())` and `leb128_write(faces.len())` directly.
The Rust `CornerTable` has **no** `NumDegeneratedFaces` / `NumIsolatedVertices` notion; it
counts every input face (incl. degenerate) in `num_faces()` (`core/corner_table/mod.rs:481`)
and every vertex in `num_vertices()`.
**C++:** `mesh_edgebreaker_encoder_impl.cc:295-301` writes
`num_vertices - NumIsolatedVertices()` and `num_faces - NumDegeneratedFaces()`; the corner
table tracks both (`corner_table.cc:150,392`). The decoder validates against these exact
adjusted counts (`mesh_edgebreaker_decoder_impl.cc:296-352`).
**Why it matters:** For any mesh with a degenerate face (two equal corner-vertices) or an
isolated vertex, the Rust-emitted `num_faces`/`num_vertices` are too large, the traversal
will not produce that many symbols, and the stream is malformed — Google's decoder will
reject it and the Rust decoder will hit `FaceCountMismatch`. In practice the encoder also
relies on the application running cleanup first (which Rust doesn't — finding #5), so a
degenerate face survives all the way into `compute_table`, where it's *skipped* for
opposite-corner setup (`mod.rs:299-304`) but still counted, guaranteeing a mismatch.

### 2. (HIGH) Encoder ignores configured traversal and the C++ auto-selection policy
**Rust:** `encode/connectivity/edgebreaker.rs:509` unconditionally emits
`EdgebreakerKind::Standard.write_to(writer)` regardless of `self.config.traversal`, and the
top-level dispatch (`encode/connectivity/mod.rs:52-66`) wires the generic over
`DefaultTraversal`/`ValenceTraversal` from `cfg.traversal` but the byte written is always 0.
So if a caller ever selects Valence, the **traversal-type byte (0=Standard) disagrees with
the actual payload** (valence per-context arrays), producing an undecodable stream.
**C++:** `mesh_edgebreaker_encoder.cc:25-68` selects Standard vs Valence based on
`speed`/tiny-mesh and writes the matching sub-method byte
(`MESH_EDGEBREAKER_STANDARD_ENCODING` / `..._VALENCE_ENCODING`) before instantiating the
corresponding impl. Default for non-tiny meshes at speed<5 is **Valence**.
**Why it matters:** Rust only ever produces Standard. That is internally consistent (its
own decoder reads Standard), but it is a different default than Google and the Valence byte
path is a latent corruption bug if ever exercised.

### 3. (MED) Sequential connectivity encode is divergent
**Rust:** `encode/connectivity/sequential.rs:80-83` writes `writer.write_u64(faces.len())`
(raw 8-byte LE), then a 1-byte method id, then direct indices. There is **no `num_points`
field**, and `Method::Compressed` (`shared/connectivity/sequential.rs`) is never emitted.
**C++:** `mesh_sequential_encoder.cc:28-78` writes `EncodeVarint(num_faces)`,
`EncodeVarint(num_points)`, then a `uint8` connectivity method (0=compressed/1=direct),
then indices sized by `num_points` (`<256`→u8, `<2^16`→u16, `<2^21`→varint, else u32).
**Why it matters:** Any sequential `.drc` Rust emits is unreadable by Google (wrong width,
missing `num_points`). The whole geometry-type byte in Rust's header is also hardcoded to
Edgebreaker (`encode/header/mod.rs:44`), so sequential is effectively dead on the encode
side and absent on the decode side (`decode/connectivity/mod.rs:60`).

### 4. (MED) Valence encoder drops start-face configs and attribute seams
**Rust:** `encode/connectivity/edgebreaker.rs:881-905` — `ValenceTraversal::encode` has
`// self.encode_start_faces();` / `// self.encode_attribute_seams();` (lines 890-891)
commented out and writes only the 6 per-context symbol blocks.
**C++:** `mesh_edgebreaker_traversal_valence_encoder.h:Done()` writes **start faces first,
then attribute seams, then** the 6 context blocks. The valence decoder
(`mesh_edgebreaker_traversal_valence_decoder.h:Start`) reads start faces + seams before the
context arrays.
**Why it matters:** The Rust Valence encoder output is missing two whole sub-streams; it is
not decodable by Google nor by the Rust valence *decoder* (which does expect start-faces +
seams first — `decode/.../edgebreaker.rs:98-99`). The valence encoder is therefore
non-functional, reinforcing #2.

### 5. (MED) No mesh cleanup; unused vertices panic instead of being removed
**Rust:** `core/corner_table/mod.rs:104-110` — `get_unused_vertices` is computed and the
constructor **panics** (`panic!("Mesh contains unused vertices ...")`) if any vertex index
in `[0, max]` is unreferenced. There is no degenerate/duplicate-face removal anywhere.
**C++:** `mesh_cleanup.cc` (`RemoveDegeneratedFaces`/`RemoveDuplicateFaces`/
`RemoveUnusedAttributes`, all default-on) is run by the application/expert-encoder layer
before corner-table construction; isolated vertices and degenerate faces are removed so the
corner table never sees them.
**Why it matters:** Real-world OBJ/glTF inputs frequently have unreferenced vertices or
degenerate triangles. Google silently cleans them; Rust panics or (per #1) emits a
malformed stream. This is a robustness/correctness gap on otherwise-valid input.

### 6. (LOW/structural) `use_single_connectivity` unimplemented
**Rust:** `encode/connectivity/edgebreaker.rs:132-134` → `unimplemented!()`.
**C++:** `mesh_edgebreaker_encoder_impl.cc:52-60` enables it when
`split_mesh_on_seams` is set or `speed >= 6`, using `CreateCornerTableFromAllAttributes`
(every distinct point becomes its own corner-table vertex) and a single shared attribute
encoder.
**Why it matters:** Mostly a missing-feature (covered below), but it is reachable by config
and would panic rather than degrade, so it is also a latent correctness/robustness issue.

### 7. (LOW) Start-face `zero_prob` rounding — verify, likely OK
**Rust:** `edgebreaker.rs:673-675` computes `zero_prob = ((freq0/total)*256.0+0.5) as u16`
then `.clamp(1,255)`. **C++:** `rans_bit_encoder.cc:83-91` computes the same raw value, sets
`zero_prob=255` only when `raw < 255` is false, then `+= (==0)`. For the high end Rust's
`clamp(.. ,255)` and Google's "stay 255 if raw≥255" agree; for zero both yield 1. I believe
these match, but the rABS *bit-ordering* on the wire (Google reverses bit groups in
`EndEncoding`; Rust writes `interior_cfg.iter().rev()`) was not verifiable without running
`google_compat`. Flagged low pending the test.

---

## Missing features

- **Mesh cleanup pipeline** (`mesh_cleanup.cc`): degenerate-face removal, duplicate-face
  removal, unused-attribute/isolated-vertex removal. Blocks: robust handling of real-world
  meshes; without it, finding #1/#5 make many inputs panic or emit malformed streams.

- **Sequential connectivity decode** (and a spec-correct encode): blocks reading/writing
  any non-edgebreaker `.drc`, and round-tripping point-cloud-style or
  compression-disabled meshes. (`decode/connectivity/mod.rs:60` stubs it out.)

- **`use_single_connectivity` / `CreateCornerTableFromAllAttributes`**: blocks the
  speed≥6 / `split_mesh_on_seams` encode mode Google selects automatically; currently
  `unimplemented!()` (panic).

- **Valence traversal end-to-end** (start-faces + seams in the encoder, finding #4):
  blocks producing the Google *default* connectivity encoding for non-tiny meshes and any
  byte-parity test of the valence path.

- **Predictive traversal** (encoder + decoder): legacy in Google, low priority, but absent.

- **Degenerate / isolated bookkeeping in `CornerTable`** (`NumDegeneratedFaces`,
  `NumIsolatedVertices`, `IsDegenerated`): prerequisite for #1 even if cleanup is added,
  because the C++ corner table still subtracts these defensively.

- **Mesh stripifier, mesh splitter, connected-components, mesh-features**: off the core
  compression path; not required for basic parity but absent.
