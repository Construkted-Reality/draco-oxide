# Attribute Prediction Schemes & Prediction Transforms — Google Parity Audit

Date: 2026-06-17
Subsystem: `src/draco/compression/attributes/` (C++) vs
`src/shared/attribute/{prediction_scheme,prediction_transform}/`,
`src/encode/attribute/`, `src/decode/attribute/` (Rust).

All Rust paths below are rooted at
`/home/outsider/Projects/Construkted_Reality/code/tileforge/draco-oxide/draco-oxide/`.
All C++ paths are rooted at
`/home/outsider/Projects/Construkted_Reality/code/tileforge/google-draco-reference/src/draco/compression/attributes/`.

## Summary

Roughly **45–55%** of the C++ prediction-scheme/transform surface is ported,
and the ported parts are largely *self-consistent* (encoder round-trips with the
Rust decoder) but only **partially bitstream-compatible** with Google Draco.
The two general-purpose paths — **single parallelogram + WRAP** (positions/UVs)
and **delta + WRAP** (generic/color) — are ported correctly and match the C++
formula and wrap math. The biggest correctness risk is the **normal/octahedral
path**: the Rust `MeshNormalPrediction` projects to octahedral coordinates with
*floating-point* math and a **hardcoded 8-bit quantization** (`1 << (8-1)`,
`max=255`/`center=127`), whereas C++ uses fully-integer canonicalization
(`CanonicalizeIntegerVector` + `IntegerVectorToQuantizedOctahedralCoords`) at the
configured quantization bits — this is not byte-compatible with Google and is
locked to one bit depth. The biggest *missing* pieces are
**constrained-multi-parallelogram** (C++ default for positions at speed 0–1; absent
in Rust), **geometric-normal decode against real Google bitstreams** (the Rust
decoder bails with `OctahedralTodo`/`PredictionSchemeTodo` on anything outside its
own emitter's narrow triple), and several schemes present only as
`unimplemented!()` stubs (multi-parallelogram `predict`, derivative prediction).
Multi-parallelogram and deprecated tex-coords are decode-only legacy in C++, so
their absence is low-impact for *encoding* but blocks decoding older streams.

## Coverage matrix

| Feature | C++ location | Rust location | Status | Notes |
|---|---|---|---|---|
| Delta / difference scheme (PREDICTION_DIFFERENCE=0) | `prediction_schemes/prediction_scheme_delta_{encoder,decoder}.h` | `shared/.../prediction_scheme/delta_prediction.rs` | **Partial/Divergent** | Rust `predict` returns the most-recently-processed vertex via `left_most_corner(last_v)` (delta_prediction.rs:74-84), not strictly `D(i-1)`. For the sequential portable path the "previous" entry is the last visited vertex, so it usually matches, but the index resolution differs from C++ (`in_data+i-num_components`, delta_decoder.h:51-59). |
| Single parallelogram (MESH_PREDICTION_PARALLELOGRAM=1) | `mesh_prediction_scheme_parallelogram_{shared,encoder,decoder}.h` | `shared/.../mesh_parallelogram_prediction.rs`; decode mirror `decode/attribute/mod.rs::predict_parallelogram_n` | **Implemented** | Formula `next + prev - opposite` correct (mesh_parallelogram_prediction.rs:262; decode mod.rs:645). Opposite-corner traversal correct (`opposite(c)`, then `next(c)/previous(c)`). Fallback differs (see finding #4). |
| Multi-parallelogram (MESH_PREDICTION_MULTI_PARALLELOGRAM=2) | `mesh_prediction_scheme_multi_parallelogram_{encoder,decoder}.h` (decode-only, back-compat) | `shared/.../mesh_multi_parallelogram_prediction.rs` | **Missing (stub)** | `predict` is `unimplemented!()` (mesh_multi_parallelogram_prediction.rs:193); `get_values_impossible_to_predict` also `unimplemented!()`. Scheme is registered in dispatch but will panic if selected. C++ encoder never emits this; decoder is back-compat only. |
| Constrained multi-parallelogram (MESH_PREDICTION_CONSTRAINED_MULTI_PARALLELOGRAM=4) | `mesh_prediction_scheme_constrained_multi_parallelogram_{shared,encoder,decoder}.h` | — | **Missing** | No Rust counterpart. This is the C++ *default* for positions at speed 0–1 (`prediction_scheme_encoder_factory.cc:99-100`). Two-pass swing (LEFT then RIGHT, cap 4), crease-bit configuration search, per-context RAns bit streams — none present. |
| Tex-coords portable (MESH_PREDICTION_TEX_COORDS_PORTABLE=5) | `mesh_prediction_scheme_tex_coords_portable_{predictor,encoder,decoder}.h` | `shared/.../mesh_prediction_for_texture_coordinates.rs` | **Partial/Divergent** | Integer geometric predictor ported (int64 + `int_sqrt`), orientation stack + RABS metadata ported. Overflow-guard set differs from C++ (finding #3); decoder side of *this scheme* only handled in encoder — decode uses parallelogram fallback for UVs (mod.rs:340-376). |
| Tex-coords deprecated (MESH_PREDICTION_TEX_COORDS_DEPRECATED=3, float) | `mesh_prediction_scheme_tex_coords_{encoder,decoder}.h` (decode-only, back-compat) | — | **Missing** | Not ported. Decode-only legacy in C++. Rust assigns id 7 to a different scheme (DerivativePrediction); id 3 is unused in Rust. |
| Geometric normal (MESH_PREDICTION_GEOMETRIC_NORMAL=6) | `mesh_prediction_scheme_geometric_normal_{encoder,decoder}.h`, `..._predictor_{base,area}.h` | `shared/.../mesh_normal_prediction.rs`; decode `decode/attribute/mod.rs::{decode_normal_attribute,predict_normal}` | **Divergent** | Area-weighted cross-product sum + `1<<29` clamp ported (mesh_normal_prediction.rs:98-113). But oct mapping is **float + hardcoded 8-bit** (finding #1) vs C++ integer `CanonicalizeIntegerVector`/`IntegerVectorToQuantizedOctahedralCoords` at configured bits. |
| Derivative prediction (no C++ equivalent) | — | `shared/.../derivative_prediction.rs` | **Missing (stub)** | Entirely `unimplemented!()` (derivative_prediction.rs:29,310). Rust-only experimental scheme, id 7. Not a Draco scheme; cannot interoperate. |
| Delta/difference transform (PREDICTION_TRANSFORM_DELTA=0) | `prediction_scheme_{encoding,decoding}_transform.h` | `encode/.../prediction_transform/difference.rs`; decode `inverse_prediction_transform/mod.rs::Difference` | **Implemented** | `corr = orig - pred`; inverse `pred + corr`. Id 0 matches. (Note: encode-side `difference.rs` additionally subtracts a global min metadata — see finding #5.) |
| Wrap transform (PREDICTION_TRANSFORM_WRAP=1) | `prediction_scheme_wrap_{transform_base,encoding,decoding}_transform.h` | `encode/.../prediction_transform/wrapped_difference.rs`; decode `inverse_prediction_transform/mod.rs::WrappedDifference` | **Implemented** | `max_diff = 1 + (max-min)`, `max_corr = max_diff/2`, even-adjust `-1`, clamp + wrap all match (wrapped_difference.rs:65-91 ≈ wrap_transform_base.h:83-96). Serializes `min` then `max` (matches encoding_transform.h:73-74). Decode uses signed add not unsigned (finding #6, low). |
| Normal octahedron transform (PREDICTION_TRANSFORM_NORMAL_OCTAHEDRON=2) | `prediction_scheme_normal_octahedron_{transform_base,encoding,decoding}_transform.h` | — | **Missing** | Rust id 2 is named `OctahedralReflection` (unused stub). No non-canonicalized octahedron transform. |
| Normal octahedron canonicalized (PREDICTION_TRANSFORM_NORMAL_OCTAHEDRON_CANONICALIZED=3) | `..._canonicalized_{transform_base,encoding,decoding}_transform.h` | `encode/.../prediction_transform/oct_orthogonal.rs`; decode `inverse_prediction_transform/mod.rs::OctahedralOrthogonalInverseTransform` | **Divergent** | Rust id 3 (`OctahedralOrthogonal`) is structurally the *canonicalized* transform: flip-inside-out + rotate-to-bottom-left + `MakePositive`, writes `max_quantized` then `center` (matches canonicalized_encoding_transform.h:68-69). But `max=255`/`center=127` hardcoded (finding #1/#2); `InvertDiamond`/`IsInDiamond`/`IsInBottomLeft` not used — a simpler flip is substituted (finding #2). |
| Reflection / pure-orthogonal transforms | (no distinct C++ types — these are Rust-internal variants) | `prediction_transform/oct_reflection.rs`, `orthogonal.rs` | **Dead/N-A** | Marked `#[allow(unused)]` "not used yet" in mod.rs:99-104. No C++ counterpart; ignore. |
| Scheme/transform method byte serialization | `sequential_integer_attribute_encoder.cc:121-125` (method int8, then transform int8) | `encode/attribute/attribute_encoder.rs:196-202` | **Implemented (order ok)** | Writes scheme id then transform id, same order. Scheme ids 0/1/2/5/6 and transform ids 0/1/3 align with C++ enums (finding #7 notes the *naming* mismatch and unused ids). |
| Scheme factory / `SelectPredictionMethod` | `prediction_scheme_encoder_factory.{h,cc}` | `encode/attribute/attribute_encoder.rs::GroupConfig::default_for` | **Divergent** | Rust hardwires per-attribute-type defaults (Position→parallelogram, Normal→geometric, TexCoord→portable, Custom/Color→delta). No speed-based selection, no `num_points<40` guard, no constrained-MP, no position-validity gate for tex/normal (factory cc:36-100). |
| Decoder factory / transform-keyed dispatch | `prediction_scheme_decoder_factory.h` (`DispatchFunctor` specialized by transform type) | `decode/attribute/mod.rs` (attribute-type + port-kind switch) | **Divergent/Partial** | Rust dispatches on `(AttributeType, PortabilizationType)` tuples, not transform-type as C++ does. Anything outside its own emitter's triples returns `PredictionSchemeTodo`/`OctahedralTodo` (mod.rs:259-261, 491-494). |

## Correctness findings

1. **[HIGH] Normal octahedral mapping is float + hardcoded 8-bit, not integer at configured bits.**
   C++ `MeshPredictionSchemeGeometricNormalPredictorArea::ComputePredictedValue`
   produces an *integer* 3D normal, then `OctahedronToolBox::CanonicalizeIntegerVector`
   forces `AbsSum == center_value` and
   `IntegerVectorToQuantizedOctahedralCoords` maps to the octahedral grid at the
   *configured* quantization bits (`normal_compression_utils.h`,
   `mesh_prediction_scheme_geometric_normal_encoder.h:117-161`). The Rust
   `MeshNormalPrediction::predict` instead calls the **float** `octahedral_transform`
   (returns `NdVector<2,f32>`), then scales by `(1 << (8-1)) - 1 = 127` with an
   explicit `// TODO: Stop hardcoding the quantization bits` and runs a bespoke
   `into_faithful_oct_quantization` corner-fixup
   (mesh_normal_prediction.rs:128-138; geom.rs:43-95,132-151). Consequences:
   (a) normals are locked to 8-bit oct quantization regardless of requested bits;
   (b) the float projection will not reproduce Google's integer-canonicalized
   octahedral indices bit-for-bit, so normal streams are not byte-compatible with
   a Google decoder, and a Google-encoded normal stream cannot be decoded
   correctly. Why it matters: normals silently lose precision / fail interop; the
   most bug-prone path in the module is implemented in the most divergent way.

2. **[HIGH] Octahedral transform omits `InvertDiamond`/`IsInDiamond`/`IsInBottomLeft`; substitutes a simpler flip.**
   C++ canonicalized encode (`..._canonicalized_encoding_transform.h:94-111`)
   does: shift to origin; if `!IsInDiamond(pred)` → `InvertDiamond(orig)` *and*
   `InvertDiamond(pred)` (the diamond reflection with the doubling + `/2`,
   `normal_compression_utils.h:216-257`); then if `!IsInBottomLeft(pred)` →
   `RotatePoint` by `GetRotationCount`. The Rust `oct_orthogonal.rs:33-58` and the
   decoder `OctahedralOrthogonalInverseTransform::inverse`
   (inverse_prediction_transform/mod.rs:152-193) replace `InvertDiamond` with a
   plain "flip inside-out" using `quadrant_sign` and **no `/2` diamond reflection**,
   and the rotate loop runs `while p0>=0 || p1>0` instead of using the discrete
   `GetRotationCount` table (canonicalized_transform_base.h:50-90). The Rust
   encode/decode are mutually inverse (self-round-trip), but they do **not**
   reproduce Google's correction values, so the octahedral correction bytes are
   not interoperable. Why it matters: normal correctness against any non-Rust peer;
   subtle because tests that only round-trip Rust↔Rust will pass.

3. **[MED] Tex-coords portable overflow-guard set diverges from C++; missing the `x_pos` pre-multiply guard placement.**
   C++ predictor (`mesh_prediction_scheme_tex_coords_portable_predictor.h:163-188`)
   guards each int64 multiply individually before computing `x_uv` and again before
   `x_pos = next_pos + (cn_dot_pn * pn)/pn_norm2_squared`. The Rust
   `mesh_prediction_for_texture_coordinates.rs:204-227` performs the guards in a
   different order and with different comparands: it checks
   `n_uv_absmax > i64::MAX / pn_norm2_squared` and
   `cn_dot_pn.abs() > i64::MAX / pn_uv_absmax` *before* `x_uv`, then
   `cn_dot_pn.abs() > i64::MAX / pn_absmax` *before* `x_pos`. When a guard trips,
   Rust falls back to `fallback_predict` (which prefers `next` then last vertex),
   whereas C++ returns `false` from the whole geometric predictor and the *scheme*
   takes its own delta fallback (predictor.h:253-277, preferring `prev`, then
   `next`, then `data_id-1`). The differing fallback preference (`next` vs `prev`)
   changes the emitted residual and the orientation-bit count on borderline inputs.
   Why it matters: divergent predictions on large-coordinate UV meshes →
   non-interoperable UV streams and possible orientation-bit desync.

4. **[MED] Parallelogram/scheme fallback predicts a different value than C++.**
   When the parallelogram is unavailable (no opposite corner, or a neighbor not yet
   processed), C++ falls back to **delta against the immediately preceding data
   entry** `in_data[(p-1)*nc]` (parallelogram_encoder.h:91-94;
   decoder.h:84-86). The Rust scheme falls back to the attribute value at
   `left_most_corner(last_v)` of the *last processed vertex*
   (mesh_parallelogram_prediction.rs:230-251), and the Rust decoder mirror uses its
   own `last_decoded` slot (decode mod.rs:625,636). Because the Rust traversal order
   (`Traverser::compute_sequence`) is not guaranteed identical to C++'s
   `data_to_corner_map` ordering, "last processed vertex" need not equal "previous
   data entry `p-1`". The Rust encoder and decoder agree with each other, so
   Rust↔Rust round-trips, but the residuals differ from Google's → not byte-
   compatible, and any ordering skew between encode and decode traversal would
   corrupt output. Why it matters: silent interop break; correctness hinges on an
   unstated traversal-order invariant.

5. **[MED] Encode-side `Difference` transform subtracts a global-min metadata that the C++ DELTA transform does not have.**
   C++ `PREDICTION_TRANSFORM_DELTA` is pure `orig - pred` with **no transform data
   serialized** (encoding_transform.h:51-63). The Rust *shared*
   `Difference` (shared/.../prediction_transform/difference.rs:40-70) and the
   encode-side variant track a per-component global minimum and subtract it in
   `squeeze`, emitting it as `FinalMetadata::Global`. If this `Difference` variant
   (transform id 0) is ever selected on the encode path that writes id 0, the
   decoder's `InverseTransform::Difference` (inverse_prediction_transform/mod.rs:98-102)
   does **plain** `pred + corr` with no min-add — an asymmetry that would corrupt
   output. In practice the default configs route generic attributes through
   `WrappedDifference` (id 1), so this mismatched `Difference` path appears
   currently unused, but it is a latent round-trip bug if id 0 is ever emitted.
   Why it matters: a live mismatch between the encode transform and its decoder
   inverse for transform id 0.

6. **[LOW] Wrap inverse uses signed add; C++ uses unsigned (`uint32`) add to avoid UB on malformed input.**
   C++ wrap decode does `out = (int32_t)(uint_pred + uint_corr)` before the
   bounds check (wrap_decoding_transform.h:56-57). Rust does signed
   `pred_clamped + corr` (inverse_prediction_transform/mod.rs:107). For well-formed
   streams the result is identical; on adversarial input Rust can panic in debug
   (overflow) or wrap differently. Low impact for trusted encode output, but a
   robustness gap against hostile bitstreams. The encode wrap math itself is
   correct (wrapped_difference.rs:65-91).

7. **[LOW] Transform/scheme enum *names* are misleading and several ids are unused or non-Draco.**
   Rust transform id 3 is named `OctahedralOrthogonal`/`Orthogonal` but is
   semantically Google's `NORMAL_OCTAHEDRON_CANONICALIZED=3`; id 2
   (`OctahedralReflection`) and id 4 (`Orthogonal`) have no Google equivalent and
   are dead (mod.rs:99-104). On the scheme side, Rust id 7 is
   `DerivativePrediction` (no Google equivalent; C++ has no method 7), and Google's
   id 3 (`TEX_COORDS_DEPRECATED`) and id 4 (`CONSTRAINED_MULTI_PARALLELOGRAM`) are
   unmapped. The ids that *do* round-trip (0,1,2,5,6 schemes; 0,1,3 transforms)
   are correct; the risk is future confusion / accidental selection of a dead id.
   Why it matters: maintainability and the chance of emitting an id the Google
   decoder rejects.

## Missing features

- **Constrained multi-parallelogram scheme (method 4).** No Rust implementation.
  Blocks: matching Google's *default* position predictor at encode speed 0–1, the
  highest-compression position path; also blocks decoding any Google stream that
  used it. Requires the two-pass LEFT/RIGHT swing, the configuration-bit search
  with `ComputeError`, the crease-flag semantics ("true = not used"), and the
  per-context reverse-order RAns bit streams.
- **Multi-parallelogram scheme `predict` (method 2).** Stub (`unimplemented!()`).
  Blocks decoding back-compat streams; will *panic* if the dispatcher ever selects
  it. Low encode impact (C++ never emits it).
- **Non-canonicalized normal octahedron transform (transform 2).** Absent. Blocks
  decoding Google normal streams that used `PREDICTION_TRANSFORM_NORMAL_OCTAHEDRON`
  (as opposed to the canonicalized variant).
- **Integer-correct octahedral toolbox** (`InvertDiamond`, `IsInDiamond`,
  `IsInBottomLeft`, `GetRotationCount`/`RotatePoint`, `ModMax`/`MakePositive`,
  `CanonicalizeIntegerVector`, `IntegerVectorToQuantizedOctahedralCoords`). Only
  partial/approximate analogues exist. Blocks byte-compatible normal encode/decode
  at arbitrary quantization bits (see findings #1, #2).
- **Speed/attribute-aware `SelectPredictionMethod`.** Rust uses fixed per-type
  defaults. Blocks reproducing Google's method choice (and therefore its exact
  bitstream) across the speed range, and the `num_points < 40` small-mesh guard.
- **Deprecated float tex-coords scheme (method 3).** Absent. Blocks decoding
  legacy UV streams (low priority; decode-only in C++).
- **Derivative prediction (Rust-only id 7).** Stub. Not a Draco scheme; cannot
  interoperate and currently dead.
