# Symbol scheme-selection fix — instrumentation, smoking gun, and fixes

**Date:** 2026-06-17. Branch `feat/google-parity`.

This is the contemporaneous record of diagnosing and fixing the encoded-size
blow-up (KNOWN_ISSUES #1), driven by instrumenting both encoders.

## 1. Instrumentation (both sides)

Added an env-gated log (`DRACO_SCHEME_LOG=1`) at the symbol-scheme decision point
in BOTH encoders:
- **draco-oxide**: `encode/entropy/symbol_coding.rs` (scratch, since removed).
- **Google C++**: an `fprintf` in `compression/entropy/symbol_encoding.cc`
  `EncodeSymbols`, built into `google-draco-reference/build_instrumented/draco_encoder`
  (Release, via cmake + make -j2). The instrumented binary is kept for future
  parity checks.

Each logs, per attribute symbol section: `num_values`, `num_components`,
`max_value`, `max_value_bit_length`, the tagged/raw bit ESTIMATES (Google), the
distinct-symbol count, and the chosen method (TAGGED vs RAW).

## 2. Smoking gun — position attribute (nc=3), Google's *logged* decision vs oxide

| mesh / qp | max_value | num_unique | G tagged_bits | G raw_bits | **Google** | **oxide (before)** | agree? |
|---|---:|---:|---:|---:|:--:|:--:|:--:|
| sphere qp11 | 2047 | 56 | 3286 | 2936 | RAW | RAW | ✓ |
| **sphere qp14** | 16383 | 58 | 4312 | 4767 | **TAGGED** | **RAW** | **✗** |
| torus qp11 | 1064 | 22 | 21545 | 18453 | RAW | RAW | ✓ |
| torus qp14 | 8496 | 101 | 39413 | 34874 | RAW | RAW | ✓ |
| bunny qp11 | 1921 | 73 | 357045 | 331102 | RAW | RAW | ✓ |
| bunny qp14 | 15375 | 452 | 627609 | 616085 | RAW | RAW | ✓ |

One red row = the whole bug. At sphere qp14, Google's estimate has
`raw_bits (4767) > tagged_bits (4312)`, so it flips to TAGGED. draco-oxide
hardcoded RAW (`attribute_encoder.rs:414`) and serialized a 16,384-entry
frequency table → 860 B (Google) vs 15,465 B (oxide), the ~18× blow-up.

Crucial detail the original KNOWN_ISSUES guess missed: the flip is driven by the
**bit-estimate comparison**, NOT the `>18`-bit threshold (`max_value_bit_len=14 <
18`). A threshold-only fix would NOT have fixed sphere.

## 3. Root causes fixed

1. **No scheme selection.** Ported Google's estimators into
   `encode/entropy/symbol_coding.rs`: `compute_bit_lengths`,
   `compute_shannon_entropy`, `approximate_rans_frequency_table_bits`,
   `approximate_tagged_scheme_bits`, `approximate_raw_scheme_bits`,
   `select_symbol_encoding_method` (tagged if `tagged < raw || max_value_bits >
   18`). `encode_symbols` now takes `Option<method>` — `None` selects, `Some`
   forces (connectivity forces raw; matches Google, keeps the connectivity decode
   path untouched). **Verified: oxide now matches Google's scheme choice on all 8
   tested mesh/qp/attribute cases.**
2. **Wrong unique count.** RAW used the *non-zero* count; now the *distinct-value*
   count (Google's `num_unique_symbols`).
3. **Bit-length off-by-one.** RAW wrote `MSB+2`; now `MSB+1` (Google's
   `unique_symbols_bit_length`, no-op cl-adjustment at the default level 7).
4. **rANS frequency-table run-length bug** (`encode/entropy/rans.rs`). The
   zero-run encoder allowed `offset==64` (overflowing `(64u8)<<2` to a 0 byte) and
   only advanced on a found non-zero, so any zero-run longer than 63 degraded to
   ONE table byte per zero — O(max_value) table instead of O(num_unique). Fixed to
   match Google's `EncodeTable`: cap offset at 63, advance `i += offset`
   unconditionally. This was the residual gap after scheme selection (torus qp14,
   sphere qp11).

## 4. Result — size parity (full mesh, oxide/google)

| | tetra | sphere | torus | bunny |
|---|---|---|---|---|
| qp11 | 4.51→**1.02×** | 2.90→**0.87×** | 1.68→**1.37×** | 1.16→**1.14×** |
| qp14 | 4.51→**1.02×** | **17.98→0.89×** | 3.01→**1.21×** | 1.19→**1.04×** |

All under the 2× ship ceiling; sphere now *beats* Google. **Interop holds**:
Google's decoder decodes every oxide output. The size-parity and interop
conformance gates both pass.

## 5. Decoder fallout (handled) — seam-split UV

Selecting TAGGED for tiny attributes exposed a pre-existing decoder bug. The
TAGGED value bitstream has no length prefix (Google appends raw value bytes), so
the decoder MUST know the exact value count. Tetrahedron's TEXCOORD is seam-split
(4 position verts → 6 UV verts); the encoder correctly writes 6 (matching
Google), but `decode/attribute/mod.rs` forced `attr_table = None` for UV and read
only 4 → reader desync → garbage quant bits → panic. (Under the old RAW scheme the
length-framed rANS buffer masked this as silently-wrong-but-aligned values.)

Fix: thread the per-attribute corner table into `decode_uv_attribute` exactly like
the normal path already does (`attr_table` is `Some` only for seam-split meshes,
so the common non-seamed interop case is unchanged). Tetra self-roundtrip now
passes.

### Still open: glTF de-seam reassembly

The glb transcoder can't yet reassemble a seam-split attribute (more verts than
positions) into a unified glTF vertex buffer (duplicate positions at seams). Three
`io::gltf::draco_decoder::tests` glb-roundtrip tests are `#[ignore]`d with a
reason pointing here. They "passed" before only because UV silently decoded to a
structurally-consistent-but-wrong 4-vert attribute. This is a separate decoder
feature, tracked for follow-up.

## 6. Verification artifacts

- `conformance/` gates: `size_parity`, `interop`, `roundtrip` (new), all green.
- `draco-oxide/src/decode/entropy/symbol_coding.rs`: added `fuzz_encode_decode_auto_select`
  (3000 random cases, both schemes).
- `encode_byte_stability.rs` fingerprints regenerated (output legitimately shrank).
- Instrumented Google encoder at `google-draco-reference/build_instrumented/`.
