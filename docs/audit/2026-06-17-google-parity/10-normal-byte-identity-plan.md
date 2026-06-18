# Normal attribute byte-identity — status + plan

## UPDATE 2026-06-18 — normal octahedral math is now BYTE-EXACT

The integer normal encode+decode is implemented (predict → CanonicalizeIntegerVector
→ ±IntegerVectorToQuantizedOctahedralCoords → compute_correction → ModMax →
min-abs-sum flip → MakePositive; decoder mirrors it with `compute_original_value`).
Verified against Google: decoding Google's own `bunny.drc` and diffing the normal
symbol arrays gives **0 differences**, and the encoder's flip-bit vector matches
Google's exactly. **bunny's encoded size now equals Google's exactly (69169 B)**;
tetra too (188 B). torus stays byte-identical; interop + round-trip + full suite green.

Two extra fixes were needed (both measured, not assumed):
- **Position quantization rounding** (`quantization_coordinate_wise.rs`): match
  Google's `Quantizer` — one f32 reciprocal `inverse_delta = max_q/range` then
  `floor(diff*inverse_delta + 0.5)`. The old `(diff/range)*max_q` rounded
  differently in f32, giving 5 off-by-one bunny vertices that cascaded into 29
  wrong normal symbols.
- i64 cross product in the area predictor (was i32, overflow risk).

**Remaining for full bunny byte-identity = the entropy layer, NOT octahedral math.**
bunny still differs in ~28 bytes: 1 byte in the position rANS stream (byte 5587)
and ~27 bytes in the flip-bit RABS buffer at the tail. The underlying values are
identical to Google (proven above); the rANS/RABS coders just choose a different
valid byte encoding. There is also a suspected oxide-internal RABS round-trip bug
on sparse bit patterns (encodes flip `[1@691]`, decodes `[1@34142]`). Next pass:
align the RABS/rANS coders to Google's exact renormalization (same class as the
rANS table-normalization fix that made torus byte-identical).

---
(original plan below)

# Normal attribute byte-identity — status + plan (original)

**Date:** 2026-06-18. Gap D, bunny's normal attribute (first divergence at byte
5587, in the NORMAL symbol stream).

## Root cause (measured, not assumed)

A subagent dumped the actual octahedral values on both sides. The suspected
parallelogram fallback was a red herring; the cause is the **normal octahedral
encoding**, in two layers:

1. **Quantization rounding (FIXED).** oxide projected the normal to float and
   **truncated** (`as i32`); Google uses **round-half-up** (`floor(x+0.5)`) plus
   an integer abs-sum canonicalization. e.g. bunny normal idx0: oxide 243.6→243,
   Google→244.
2. **Prediction + correction transform (NOT yet fixed).** The flip selection and
   the correction transform diverge structurally (see below).

## Done

- New `shared/attribute/octahedron_toolbox.rs`: a faithful port of Google's
  integer `OctahedronToolBox` (`normal_compression_utils.h`) — quantization
  (`float_vector_to_quantized_octahedral_coords`, `integer_vector_to_quantized_octahedral_coords`,
  `canonicalize_octahedral_coords`) AND the correction/flip half
  (`compute_correction` = the canonicalized encoding transform's `ComputeCorrection`,
  plus `mod_max`, `make_positive`, `is_in_diamond`, `invert_diamond`,
  `get_rotation_count`, `rotate_point`, `is_in_bottom_left`,
  `canonicalize_integer_vector`).
- Wired the **quantization** half into `encode/attribute/portabilization/octahedral_quantization.rs`.
  Verified: bunny's portable oct values now byte-match Google (idx0 244, idx2 240,
  idx5 198, idx6 217, idx9 252). torus stays byte-identical; interop + round-trip green.
  Output moved toward Google (bunny 66557→68192 B, vs Google 69169).

## Remaining (the restructure) — why it's not surgical

oxide's normal encode is a `predict → transform` split where the transform
(`shared/attribute/prediction_transform/oct_orthogonal.rs`,
`OctahedronOrthogonalTransform`) is **entirely float-based**: `Correction =
NdVector<2, f64>`, it operates on the 3D normals via float `octahedral_transform`
and pushes `orig_oct − pred_oct` (float). And `MeshNormalPrediction::predict`
(`mesh_normal_prediction.rs:128-150`) re-does the float oct quantization for the
*predicted* value and selects the flip by a **float dot-product**.

Google instead does it all integer and **coupled**, in
`MeshPredictionSchemeGeometricNormalEncoder::ComputeCorrectionValues`
(`mesh_prediction_scheme_geometric_normal_encoder.h:95-160`), per data entry:

1. `predictor_.ComputePredictedValue(corner)` → integer 3D normal (area-weighted
   sum of incident face normals). oxide's area sum (`predict` lines 100-118) is
   already integer and matches `predictor_area.h` — KEEP it.
2. `CanonicalizeIntegerVector(pred_3d)`  (toolbox, ready).
3. `IntegerVectorToQuantizedOctahedralCoords(pred_3d)` → `pos_pred_oct`;
   negate → `neg_pred_oct`  (toolbox, ready).
4. `ComputeCorrection(orig=portable_oct, pos_pred_oct)` and `(…, neg_pred_oct)`
   (toolbox `compute_correction`, ready).
5. `ModMax` each component; pick the direction with smaller `AbsSum` → that's the
   **flip bit**; final `out_corr = MakePositive(chosen)`  (toolbox, ready).
6. Flip bits → a `RAnsBitEncoder` block (oxide already emits a RABS flip block in
   `encode_prediction_metadtata`).

So the flip selection is a function of the *correction*, which the float
predict→transform split can't reproduce. The fix is to **replace the normal
prediction+transform path** with a direct port of `ComputeCorrectionValues`
using the (already-built) toolbox, emitting the integer corrections as the symbol
stream and the flip bits via the existing RABS path. The decoder's normal path
(`decode/attribute/predict_normal`) must invert it (Google's
`ComputeOriginalValue` in the *decoding* transform + `GeometricNormalPredictor`).

### Effort
- Encoder: rewrite `MeshNormalPrediction::predict` (or its driver) to the coupled
  algorithm above; make the symbol = the integer correction (not the float
  `NdVector<2,f64>`). This touches the `Correction` type for the normal transform
  and how the attribute encoder extracts symbols from it — the main friction.
- Decoder: matching integer `ComputeOriginalValue` + predictor so round-trip and
  Google-decode hold.
- Verify: `bytediff bunny` should pass byte 5587 (and ideally reach BYTE
  IDENTICAL); interop + round-trip + byte_identity gate stay green.
- Risk: moderate (octahedral math is fiddly; `InvertDiamond` uses unsigned
  wrapping — ported but only exercised once the correction path is wired).

This is a focused next pass, not a one-liner — best done with fresh context.
