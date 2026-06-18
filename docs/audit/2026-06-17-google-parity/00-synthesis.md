# draco-oxide ↔ Google C++ Draco — parity audit synthesis

**Date:** 2026-06-17
**Reference:** Google Draco C++ `main` HEAD `8c1f17b` (cloned to `../google-draco-reference`)
**Method:** 5 independent read-only review agents, one per subsystem. Per-subsystem
detail in the sibling `01`–`05` files; this file is the combined coverage matrix
and the prioritized gap list.

---

## 1. Headline

The **core mesh happy-path is real and self-consistent**: encode → decode of
positions / normals / UVs / colors through Standard Edgebreaker at default
settings round-trips, and the decoder is verified bit-perfect against Google on
the bundled fixtures (tetrahedron/sphere/torus/bunny).

But measured against the **full** Google library, coverage is roughly **25–35% by
surface area**, and — more importantly — several **shipped, non-default paths are
self-consistent but NOT byte-compatible with Google's decoder**, which is in
tension with the README's blanket "bit-perfect" claim. The claim holds for the
*specific tested fixtures* (default settings, position/Standard-Edgebreaker), not
across the option space.

### Per-subsystem coverage (agent estimates)

| Subsystem | Ported | Notes |
|---|---:|---|
| Connectivity & mesh topology | ~60–70% | Standard Edgebreaker faithful; Valence end-to-end dropped; no mesh cleanup |
| Attribute prediction & transforms | ~45–55% | Single-parallelogram + WRAP correct; normals & scheme-selection diverge |
| Entropy / bit-coders / quantization | ~55–60% | Coders faithful; **entire scheme-selection layer missing** |
| Point cloud compression | ~0% | Data structures are empty stubs |
| Metadata | ~10% | Encode is a stub; decode framing wrong (u8 vs varint) |
| Animation | 0% | Absent |
| File I/O (OBJ/PLY/STL/…) | ~8% | OBJ *read* only |
| glTF / transcoder | ~30% | One-way GLB Draco compress + Draco-strip decode |
| Scene | ~25% | Data model only, never wired, stubbed transform |
| Config / presets | ~10% | No speed/level/method surface |
| Public / expert API | ~20% | `encode()`/`decode()` only; no Encoder/ExpertEncoder |

---

## 2. The "self-consistent but not Google-compatible" theme

This is the through-line across three agents. The Rust encoder and decoder agree
with *each other* in these paths, so Rust→Rust round-trip tests pass, but the
bytes differ from Google's format so a Google decoder would reject or misread
them (and vice-versa):

- **Entropy scheme selection** — Rust always `DirectCoded`; Google picks tagged vs raw. (03)
- **Normal / octahedral transform** — float + hardcoded 8-bit; missing `InvertDiamond`/integer toolbox. (02)
- **Parallelogram / texcoord fallback** — predicts a different neighbor value than C++. (02)
- **Sequential connectivity** — raw `u64` counts, no varint, no method byte. (01)
- **Metadata** — `u8` length prefixes instead of Draco's varint/`EncodeString` framing. (04)

Because the cross-decoder test (`draco_decoder = 0.0.26` cxx bridge) currently
**fails to link** (`-ldraco` missing — agent 01 could not run `google_compat`/
`round_trip`), none of these divergences are currently caught by CI. Fixing that
link is cheap and unblocks confidence in everything else.

---

## 3. Prioritized gap list

Ordered by Adrian's rules: dependency order → verification difficulty → risk of
subtle bugs → blast radius. **Not** by implementation effort.

### Tier 0 — Correctness bugs in already-shipped paths

These affect output that tileforge produces *today*.

1. **Entropy scheme selection / size-blowup** *(03, HIGH)* — root cause CONFIRMED
   and re-attributed: not table sizing, but the total absence of tagged-vs-raw
   estimation. Rust hardcodes `DirectCoded`
   (`encode/attribute/attribute_encoder.rs:414`,
   `encode/entropy/symbol_coding.rs:28`); Google's `EncodeSymbols`
   (`symbol_encoding.cc:134-158`) estimates both and forces tagged when
   `max_value_bit_length > 18`. Directly causes the ~2× blowup at 14-bit that
   made Draco a net loss for tileforge. Sub-bugs in the same file: "unique
   symbols" counts non-zeros not distinct values (`symbol_coding.rs:46`), and a
   `MSB+2` vs Google's `MSB+1` off-by-one bit-length (`symbol_coding.rs:113`).
   **Verification:** directly measurable — re-encode a primitive at 11 vs 14-bit
   and compare byte size to Google. *Blast radius: every encode.*

2. **Degenerate faces / isolated vertices not subtracted** *(01, HIGH)* — Rust
   writes raw `num_vertices()` / `faces.len()`
   (`encode/connectivity/edgebreaker.rs:514-515`); Google subtracts
   `NumIsolatedVertices()` / `NumDegeneratedFaces()`
   (`mesh_edgebreaker_encoder_impl.cc:295-301`). Produces a malformed/undecodable
   stream on such meshes. Related: Rust **panics** on unused vertices
   (`core/corner_table/mod.rs:106`) because mesh cleanup (`mesh_cleanup.cc`) is
   absent. *Blast radius: any real-world dirty mesh.*

3. **Edgebreaker traversal-type byte can disagree with payload** *(01 + 05, HIGH)*
   — encoder always emits `EdgebreakerKind::Standard`
   (`edgebreaker.rs:509`) regardless of config while dispatching a Valence
   payload; Valence `encode()` also drops start-faces + attribute seams
   (`edgebreaker.rs:890-891`), so Google's default connectivity mode is
   non-functional. Encoding method is not selectable and sequential streams are
   rejected on decode (`decode/connectivity/mod.rs:58`).

### Tier 1 — Byte-compatibility divergences (interop with real Draco)

4. **Normal / octahedral path** *(02, HIGH)* — float `octahedral_transform` +
   hardcoded `127` (`mesh_normal_prediction.rs:128-138`, `geom.rs:43-95`);
   missing `InvertDiamond`/`IsInDiamond`/`RotatePoint` integer toolbox
   (`oct_orthogonal.rs:33-58`). Not byte-compatible; locked to 8-bit.
5. **Parallelogram / texcoord fallback value** *(02, MED)* — falls back to
   `left_most_corner(last_v)` vs C++'s preceding data entry `p-1`
   (`mesh_parallelogram_prediction.rs:230-251`).
6. **Metadata framing** *(04)* — encode is a stub writing `u32 0`
   (`encode/metadata/mod.rs:12`); decode uses `u8` lengths not varint, so it
   cannot parse genuine Draco metadata, and the result is discarded
   (`decode/mod.rs:44`).
7. **Sequential connectivity wire format** *(01)* — diverges from Google's
   varint+method-byte framing; decode side is a stub.

### Tier 2 — Latent panics (will crash if dispatched)

8. `unimplemented!()` stubs that look functional but panic: multi-parallelogram
   `predict` and derivative prediction (02); `use_single_connectivity` /
   `CreateCornerTableFromAllAttributes` (`edgebreaker.rs:133`, Google's speed≥6
   default); sequential attribute output (`attribute_encoder.rs:280`).

### Tier 3 — Missing major subsystems

9. **Constrained multi-parallelogram (method 4)** *(02)* — entirely absent; this
   is C++'s *default* position predictor at speed 0–1.
10. **Speed / compression-level (0–10) + method-selection surface** *(05)* —
    `Config` has no level field; `DracoCompressionOptions` per-type defaults
    (color/tangent/weight = 8-bit) not honored.
11. **Point cloud compression** *(04)* — entire `PointCloud` + KD-tree codec
    absent (~4k C++ LoC). Data-structure stubs are 1-byte empty files.
12. **Animation / skinning** *(04)* — absent.
13. **Structural metadata / property tables** *(04)* — ~2.2k C++ LoC, absent
    (largest single missing chunk).
14. **File I/O** *(05)* — PLY/STL/OBJ-write/point-cloud-IO absent; glTF is
    one-directional GLB-only; scene model inert.

### Tier 4 — Infra / verification (cheap, unblocks confidence)

15. **Fix the `draco_decoder` cxx dev-dep link** so `google_compat`/`round_trip`
    actually run in CI (01). Without it, every byte-compatibility claim above is
    unverified.

---

## 4. Notable corrections to existing docs

- **`KNOWN_ISSUES.md` #1 mis-attributes the size-blowup.** The suspected
  `max_symbol+1` cause is wrong — that sizing is correct and identical on both
  sides (`symbol_encoding.cc:250` ↔ `symbol_coding.rs:149-157`). Real cause is
  missing scheme selection. Doc should be updated.
- **`CLAUDE.md`** still calls the crate `draco-rs` and references a `draco-rs/`
  dir and "decoder largely incomplete" — all stale; crate is `draco-oxide` and
  the decoder is functional.
- **`README.md`** "bit-perfect against Google" should be scoped to the tested
  fixtures/default settings, given the Tier-1 divergences.
