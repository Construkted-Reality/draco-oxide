# Known Issues

## 1. Encoded size blows up with quantization bit-depth (entropy coder)

**Status:** open, not fixed. Documented 2026-06-17.
**Severity:** high — makes Draco compression net-useless or counterproductive at
the quantization bit-depths real pipelines use (12–14 bit position).

### Symptom

The encoded size of an attribute grows *pathologically* with its quantization
bit-depth. There is a large, roughly **fixed per-primitive overhead that scales
with the number of quantization bits**, far beyond the ~1 bit/component/vertex
that the extra precision should cost.

Measured per-primitive fixed overhead (position attribute):
- **~760 bytes/primitive at 11-bit**
- **~12,900 bytes/primitive at 14-bit**  (~17× worse for +3 bits)

A degenerate **3-vertex / 1-triangle primitive encodes to 5,000–16,000 bytes.**

### Evidence — same mesh, draco-oxide vs Google C++ `draco_encoder` 1.5.7

Single triangle-mesh primitive, 2,492 vertices / 2,410 triangles, POSITION (VEC3
f32) + TEXCOORD_0 (VEC2 f32), `qt=10`, compression level 7:

| position bits (`qp`) | Google C++ libdraco | draco-oxide |
|---:|---:|---:|
| 11 | 8,962 B | 8,686 B |
| 12 | 9,685 B | — |
| 13 | 10,412 B | — |
| 14 | **11,146 B** | **21,720 B** |

Google scales smoothly (+0.88 B/vertex over 11→14, ~728 B/bit — exactly the
expected entropy increase). draco-oxide **matches Google at 11-bit (even 3 %
smaller)** but balloons to **~2× Google at 14-bit**, spending ~10.5 KB it
shouldn't. So the base encoder is competitive; only the bit-depth scaling is
broken. This is **draco-oxide-specific** — not inherent to the Draco format.

### Suspected root cause

`src/encode/entropy/symbol_coding.rs`:
- The `DirectCoded` path (`encode_symbols_direct_coded` →
  `encode_symbols_direct_coded_precision_unwrapped`) builds `freq_counts` sized by
  **`max_symbol + 1`** — i.e. by the largest symbol *value*, not the count of
  distinct symbols. Higher quantization bits ⇒ larger residual values ⇒ a much
  larger frequency/distribution table, serialized in `RansSymbolEncoder::new`
  (`src/encode/entropy/rans.rs`, the distribution-encoding loop ~lines 218–254).
- Google libdraco estimates **both** the tagged (length-coded) and raw (direct)
  symbol schemes and emits whichever is smaller
  (`compression/entropy/symbol_encoding.cc`). draco-oxide's
  `SymbolEncodingMethod` selection (top of `symbol_coding.rs`) likely chooses or
  implements the wrong scheme as residual ranges grow.

Hypotheses to confirm before fixing (instrument first):
1. Build the `analyzer` (`cargo build --bin analyzer --features evaluation`) and
   compare the per-section byte breakdown of one primitive at 11-bit vs 14-bit —
   confirm the bytes are in the symbol distribution table / chosen scheme.
2. Log which `SymbolEncodingMethod` is chosen per attribute and compare to what
   Google picks for the same data.

### Reproduce

```bash
# Google baseline (reference): tools draco_encoder/draco_decoder 1.5.7
draco_encoder -i mesh.obj -o g11.drc -qp 11 -qt 10 -cl 7   # ~9 KB
draco_encoder -i mesh.obj -o g14.drc -qp 14 -qt 10 -cl 7   # ~11 KB  (smooth)

# draco-oxide: encode the same mesh at qp 11 vs 14 and compare the
# KHR_draco_mesh_compression bufferView byteLength per primitive.
# 11-bit ≈ Google; 14-bit ≈ 2× Google.
```

### Impact / workaround

Downstream (tileforge-optimize) raised POSITION to 14-bit for spatial precision
and saw Draco geometry roughly triple, becoming a net loss on tilesets with many
small primitives. Until fixed, callers must either keep bits low (≤11, at the
cost of precision) or skip Draco when its output exceeds the raw geometry.

> Note: a separate, already-fixed bug (zero-anchored quantization bbox) lives on
> branch `fix/quantization-tight-bbox`; it is unrelated to this size blowup.
