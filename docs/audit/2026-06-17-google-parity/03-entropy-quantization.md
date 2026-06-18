# Google Draco Parity Audit — Entropy coding, bit coders & quantization

Date: 2026-06-17
Subject: `draco-oxide` (Rust port) vs `google-draco-reference` (C++ HEAD)
Scope: `compression/entropy/`, `compression/bit_coders/`, `core/quantization_utils.*`, `core/bounding_box.*`

> Path note: the Rust crate source root is
> `draco-oxide/draco-oxide/src/` (the repo has a nested `draco-oxide/`
> directory). All Rust `file:line` references below are relative to that root.

## Summary

Roughly **55–60%** of the C++ entropy/quantization surface is implemented in
Rust. The core rANS symbol encoder/decoder, the RABS binary coder, both the
tagged ("length-coded") and direct ("raw") symbol schemes, and the
coordinate-wise quantizer/dequantizer math are present and numerically faithful.
**The size-blowup hypothesis is CONFIRMED, but the root cause is slightly
different from the one stated in the brief.** The brief blamed DirectCoded
sizing its frequency table by `max_symbol+1`; that table sizing is actually
*correct* — C++ does exactly the same (`symbol_encoding.cc:250`,
`frequencies(max_entry_value + 1, 0)`). The real defect is that **the Rust
encoder performs no scheme selection at all.** The entire size-estimation layer
that C++ uses to choose between tagged and raw — `ApproximateTaggedSchemeBits`,
`ApproximateRawSchemeBits`, `ComputeShannonEntropy`,
`ApproximateRAnsFrequencyTableBits`, and the `max_value_bit_length >
kMaxRawEncodingBitLength` guard — is entirely **missing**. The Rust caller
hardcodes `SymbolEncodingMethod::DirectCoded` (`encode/attribute/attribute_encoder.rs:414`),
and `encode_symbols` carries a literal `// ToDo: Add the logic to dynamically
determine the config` (`encode/entropy/symbol_coding.rs:28`). As quantization
bit-depth rises, residual ranges widen, the raw frequency table's run-length /
zero-frequency cost (`8*unique + 8*(max_value-unique)/64`) grows, and C++ would
switch to the tagged scheme — but Rust never does, so it pays the exploding raw
table. That is the 11-bit-matches / 14-bit-2x divergence. Two secondary defects
in the same DirectCoded path (a wrong unique-symbol count and an off-by-one
bit-length) further inflate the chosen rANS precision.

## Coverage matrix

| Feature | C++ location | Rust location | Status | Notes |
|---|---|---|---|---|
| Scheme selection (estimate tagged vs raw, pick smaller) | `entropy/symbol_encoding.cc:116-158` | — (none) | **Missing** | Caller hardcodes DirectCoded; `// ToDo` at `encode/entropy/symbol_coding.rs:28`. Root cause of size blowup. |
| `ApproximateTaggedSchemeBits` | `entropy/symbol_encoding.cc:75-91` | — | **Missing** | No tagged-size estimate. |
| `ApproximateRawSchemeBits` | `entropy/symbol_encoding.cc:93-103` | — | **Missing** | No raw-size estimate. |
| `ComputeShannonEntropy` | `entropy/shannon_entropy.cc:10-35` | — | **Missing** | No entropy estimator anywhere in Rust (`grep shannon` → 0 hits). |
| `ApproximateRAnsFrequencyTableBits` | `entropy/rans_symbol_coding.h:42-49` | — | **Missing** | Table-cost model absent. |
| `ShannonEntropyTracker` (incremental) | `entropy/shannon_entropy.cc:54-145` | — | **Missing** | Used by C++ adaptive prediction scoring; absent. |
| `ComputeBinaryShannonEntropy` | `entropy/shannon_entropy.cc:37-52` | — | **Missing** | — |
| Tagged ("length-coded") symbol encode | `entropy/symbol_encoding.cc:174-243` | `encode/entropy/symbol_coding.rs:61-103` | **Implemented** | Reachable only if caller passes `LengthCoded`; no caller does for attributes. |
| Tagged symbol decode | `entropy/symbol_decoding.cc:52-88` | `decode/entropy/symbol_coding.rs:47-80` | **Implemented** | LSB-first value bits matched. |
| Raw/direct symbol encode | `entropy/symbol_encoding.cc:245-374` | `encode/entropy/symbol_coding.rs:105-170` | **Partial/Divergent** | Table sizing matches C++; precision-selection inputs are wrong (findings 2 & 3). |
| Raw/direct symbol decode | `entropy/symbol_decoding.cc:90-179` | `decode/entropy/symbol_coding.rs:82-130` | **Implemented** | Self-describing bit_length read; round-trips. |
| rANS symbol encoder (table build, normalization) | `entropy/rans_symbol_encoder.h:86-251` | `encode/entropy/rans.rs:135-281` | **Implemented** | Probability rescale + zero-freq RLE table format matched. Over-allocation rebalance uses a simpler loop (finding 4). |
| rANS symbol decoder | `entropy/rans_symbol_decoder.h` | `decode/entropy/rans.rs` | **Implemented** | Mirrors table format. |
| rANS precision formula `(3*bitlen/2)` clamp[12,20] | `entropy/rans_symbol_coding.h:26-38` | hardcoded per-arm in `symbol_coding.rs:116-133` | **Implemented** | Hardcoded constants match the formula for all bit_lengths 1–18. |
| rANS state renorm / write / flush | `entropy/ans.h` (`rans_write`, `write_end`) | `encode/entropy/rans.rs:34-72` | **Implemented** | 4-way LEB-style flush byte matches. |
| RABS binary coder (rans_bit) | `bit_coders/rans_bit_encoder.cc`, `rans_bit_decoder.cc` | `encode/entropy/rans.rs:75-133` (`RabsCoder`), `decode/entropy/rans.rs` | **Implemented** | Used for octahedral normal flip bits + RLE. |
| Adaptive rANS bit coder | `bit_coders/adaptive_rans_bit_encoder.cc` / `_decoder.cc` | — | **Missing** | No adaptive (running-probability) binary coder. |
| Direct bit coder | `bit_coders/direct_bit_encoder.cc` / `_decoder.cc` | `core/bit_coder.rs` (`BitWriter`/`BitReader`) | **Implemented** | Functionally equivalent LSB/MSB bit packing. |
| Symbol bit coder | `bit_coders/symbol_bit_encoder.cc` | folded into `core/bit_coder.rs` | **Partial** | No standalone type; equivalent path exists. |
| Folded integer bit coder | `bit_coders/folded_integer_bit_encoder.h` | — | **Missing** | — |
| Quantizer (range → int) | `core/quantization_utils.cc:19-25`, `.h:42-55` | `encode/attribute/portabilization/quantization_coordinate_wise.rs:109-130` | **Implemented** | `floor(val*max_q/range + 0.5)` algebraically matched (finding 5 nuance). |
| Dequantizer | `core/quantization_utils.cc:27-40` | `decode/attribute/portabilization/dequantization_coordinate_wise.rs` (+ `super::Quantization`) | **Implemented** | `val*range/max_q`. |
| Octahedral quantization | (transform/normal) | `encode/attribute/portabilization/octahedral_quantization.rs` | **Implemented** | Present. |
| Quantization range / tight bbox derivation | AttributeQuantizationTransform (`range = max component delta`) | `quantization_coordinate_wise.rs:47-92` | **Implemented** | Tight per-mesh bbox; seeds min/max from first vertex (recent fix). |
| `BoundingBox` type | `core/bounding_box.cc/.h` | — (no `BoundingBox` type) | **Missing** | Range computed inline; no reusable AABB type. Low impact. |

## Correctness findings

### 1. [CRITICAL] No tagged-vs-raw scheme selection — root cause of the bit-depth size blowup

- **C++**: `EncodeSymbols` always estimates *both* schemes and emits whichever is
  smaller, and forces tagged when the value width exceeds the raw cap:
  `symbol_encoding.cc:134-158` —
  ```
  const int64_t tagged_scheme_total_bits = ApproximateTaggedSchemeBits(...);
  const int64_t raw_scheme_total_bits   = ApproximateRawSchemeBits(...);
  const int max_value_bit_length = MostSignificantBit(max(1u,max_value)) + 1;
  if (tagged_scheme_total_bits < raw_scheme_total_bits ||
      max_value_bit_length > kMaxRawEncodingBitLength /*18*/) method = TAGGED;
  else method = RAW;
  ```
  The raw table cost it weighs is
  `ApproximateRAnsFrequencyTableBits(max_value, num_unique)` =
  `8*unique + 8*(unique + (max_value-unique)/64)` (`rans_symbol_coding.h:42-49`),
  which grows with `max_value` (the largest residual value), i.e. with
  quantization bit-depth.
- **Rust**: No estimation exists. `encode_symbols` (`encode/entropy/symbol_coding.rs:18-50`)
  takes the method as a parameter with `// ToDo: Add the logic to dynamically
  determine the config` (`:28`), and the attribute encoder hardcodes
  `SymbolEncodingMethod::DirectCoded` (`encode/attribute/attribute_encoder.rs:414`).
  The edgebreaker connectivity path also hardcodes DirectCoded
  (`encode/connectivity/edgebreaker.rs:901`).
- **Why it matters**: This is the reported "matches at 11-bit, ~2x at 14-bit"
  bug. At low bit-depth, residuals are small, `max_value` is small, the raw
  table is cheap, and raw happens to be the scheme C++ would also pick — so the
  outputs match. As bit-depth climbs, `max_value` (and the raw zero-frequency
  run-length cost) explodes; C++ flips to the tagged scheme (or is forced to by
  the `> 18` guard), but Rust stays on raw and pays the full blown-up table.
  The brief's specific hypothesis ("table sized by `max_symbol+1`") is a
  **red herring**: that sizing is correct and identical on both sides
  (`symbol_encoding.cc:250` vs `encode/entropy/symbol_coding.rs:149-157`). The
  true fix is to port the estimator + selector. **Refuted sub-claim; confirmed
  overall blowup with a different precise cause.**

### 2. [HIGH] DirectCoded uses non-zero count instead of distinct-symbol count

- **C++**: `num_unique_symbols` is the number of *distinct* symbol values,
  produced by `ComputeShannonEntropy` (`shannon_entropy.cc:13-32`), then fed to
  `EncodeRawSymbols` to derive `unique_symbols_bit_length`
  (`symbol_encoding.cc:277-281`).
- **Rust**: `let num_symbols = symbols.iter().filter(|&&x| x > 0).count();`
  (`encode/entropy/symbol_coding.rs:46`) — this counts how many symbols are
  *non-zero*, which is neither the distinct-value count nor anything C++
  computes. It is then passed as `num_unique_symbols` into the precision
  selector.
- **Why it matters**: The value drives `bit_length` →`RANS_PRECISION` selection
  (`symbol_coding.rs:113`). It does not break round-trip (the chosen bit_length
  is written to the stream and read back, `decode/entropy/symbol_coding.rs:87`),
  but it systematically picks the wrong rANS precision vs Google, changing the
  bitstream and usually inflating the frequency table / precision. Compounds
  finding 1's size blowup.

### 3. [MEDIUM] Off-by-one in DirectCoded bit-length, plus missing compression-level adjustment

- **C++**: `symbol_bits = MostSignificantBit(num_unique); unique_symbols_bit_length
  = symbol_bits + 1`, then a compression-level adjustment (default level 7 → no
  change; levels <4/<6/>7/>9 shift it ±1/±2) and a clamp to `[1,18]`
  (`symbol_encoding.cc:277-311`). `MostSignificantBit(n)` is the **0-based**
  highest-set-bit index (`core/bit_utils.h:58-60`, `31 ^ __builtin_clz`), so for
  `n` the result is `MSB+1` significant... i.e. `unique_symbols_bit_length =
  bitwidth(n)`.
- **Rust**: `let bit_length = (64 - num_unique_symbols.leading_zeros() as usize +
  1).clamp(1, 18);` (`encode/entropy/symbol_coding.rs:113`). `64 -
  leading_zeros` already equals the bit-width `= MSB+1`; the extra `+ 1` makes it
  `MSB+2` — **one too high** vs C++. The compression-level adjustment is also
  entirely absent.
- **Why it matters**: Selects a higher rANS precision than Google for the same
  data → larger frequency table, larger output. Self-describing so it still
  round-trips, but diverges from Google byte-for-byte and adds to the size
  regression. Cheap to verify (single expression).

### 4. [LOW] rANS table over-allocation rebalancing uses a simpler algorithm

- **C++**: When `total_rans_prob > rans_precision`, rescales proportionally
  (`new_prob = floor(rel_error * prob)`, with guards against emptying the most
  frequent symbol) (`rans_symbol_encoder.h:135-171`).
- **Rust**: Decrements one unit at a time from the largest probabilities,
  wrapping around (`encode/entropy/rans.rs:197-213`, `// ToDo: Do better
  discrete normalization`).
- **Why it matters**: Both reach a valid table summing to `rans_precision`, so
  decoding is correct, but the resulting probability distribution (and thus
  compressed size and exact bytes) can differ slightly from Google. Not a
  correctness break; a small-size / bit-exactness divergence.

### 5. [INFO] Quantizer math is faithful; one rounding-direction nuance

- **C++**: `QuantizeFloat` = `floor(val * inverse_delta + 0.5)`, `inverse_delta =
  max_quantized_value / range`, `max_quantized_value = (1<<bits)-1`
  (`quantization_utils.cc:21-25`, `.h:47-50`).
- **Rust**: `floor((diff/range_size)*((1<<bits)-1) + 0.5)`
  (`quantization_coordinate_wise.rs:124-128`). Algebraically identical because
  `diff = val - min_values >= 0`, so the operand of `+0.5` is always
  non-negative and the `to_i64()` truncation equals `floor`. No divergence for
  in-range data. (If a residual transform ever fed a negative operand here, C++
  `floor` and Rust truncate-toward-zero would differ — not currently reachable.)
  The tight-bbox range derivation (`range = max component delta`, seeded from the
  first vertex) matches the C++ AttributeQuantizationTransform.

## Missing features

- Scheme-selection / size-estimation layer: `ApproximateTaggedSchemeBits`,
  `ApproximateRawSchemeBits`, `ComputeShannonEntropy`,
  `ApproximateRAnsFrequencyTableBits`, and the `max_value_bit_length > 18`
  forced-tagged guard. (Finding 1 — top priority.)
- `ShannonEntropyTracker` / `ComputeBinaryShannonEntropy` (incremental entropy,
  used by C++ for adaptive prediction scoring).
- Adaptive rANS bit coder (`adaptive_rans_bit_encoder/decoder`) — the
  running-probability binary coder. Only the static RABS coder exists.
- Folded integer bit coder (`folded_integer_bit_encoder.h`).
- Standalone symbol bit coder type (functionality is folded into
  `core/bit_coder.rs`).
- Compression-level option plumbing for raw symbol coding
  (`symbol_encoding_compression_level`, `symbol_encoding_method` overrides).
- `BoundingBox` core type (range computed inline; low impact).
