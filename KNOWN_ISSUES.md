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

### Root cause (CONFIRMED 2026-06-17 via C++/Rust parity audit)

**The original hypothesis below was WRONG and is corrected here.** The audit
(`docs/audit/2026-06-17-google-parity/03-entropy-quantization.md`) read both
codebases side by side and refuted the `max_symbol + 1` theory.

- **The `max_symbol + 1` table sizing is correct and identical on both sides.**
  Google does `frequencies(max_entry_value + 1, 0)`
  (`compression/entropy/symbol_encoding.cc:250`); draco-oxide does the same
  (`src/encode/entropy/symbol_coding.rs:149-157`). This is NOT the cause.
- **The real cause: draco-oxide runs no symbol-scheme selection at all.** It
  hardcodes the `DirectCoded` (raw) scheme
  (`src/encode/attribute/attribute_encoder.rs:414`; literal
  `// ToDo: Add the logic to dynamically determine the config` at
  `src/encode/entropy/symbol_coding.rs:28`). Google's `EncodeSymbols`
  (`compression/entropy/symbol_encoding.cc:134-158`) estimates **both** the
  tagged (length-coded) and raw (direct) schemes and emits the smaller, and
  **forces tagged when `max_value_bit_length > 18`**. The whole estimator stack
  (`ApproximateTaggedSchemeBits`, `ApproximateRawSchemeBits`,
  `ComputeShannonEntropy`, `ApproximateRAnsFrequencyTableBits`) is absent in Rust
  (`grep shannon` → 0 hits).
- At 11-bit, residuals are small so Google also picks raw → outputs match. At
  14-bit, `max_value` grows, the raw scheme's run-length table cost explodes,
  Google flips to tagged, draco-oxide does not → ~2× blowup.

Two compounding sub-bugs in the same file (also confirmed):
1. "Unique symbols" is computed as the **non-zero** count
   (`symbols.iter().filter(|&&x| x > 0).count()`, `symbol_coding.rs:46`), not the
   **distinct-value** count Google derives — drives wrong rANS precision.
2. Bit-length is `MSB + 2` (`symbol_coding.rs:113`) vs Google's `MSB + 1`
   (`symbol_encoding.cc:279-281`); also omits the compression-level adjustment.
   Picks higher precision than Google → larger tables.

### Fix direction

Implement Google's scheme selection: estimate tagged vs raw bits, emit the
smaller, force tagged above the 18-bit threshold. Fix the unique-symbol count and
the `MSB + 1` bit-length. Verify with the `conformance/` size-parity harness
(re-encode a primitive at 11 vs 14-bit and compare the byte size to Google).

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
