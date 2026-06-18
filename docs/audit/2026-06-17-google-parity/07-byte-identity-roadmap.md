# Byte-identity roadmap — draco-oxide encoder vs Google C++

**Date:** 2026-06-17. Goal: byte-identical encoder output vs Google `draco_encoder`
1.5.7 at matched settings. Draco is deterministic, so this is achievable.

## Tooling

- `cargo run -p conformance --release --bin bytediff -- <mesh> <qp>` — prints
  sizes + first-divergence offset + hex on both sides.
- Instrumented Google encoder: `google-draco-reference/build_instrumented/draco_encoder`
  (`DRACO_SCHEME_LOG=1` logs symbol-scheme decisions).
- Draco mesh header is 11 bytes: `"DRACO"`(5) + major(1) + minor(1) +
  encoder_type(1,=1 mesh) + method(1,=1 edgebreaker) + flags(2).

## Status of first-divergence by mesh (qp11)

| mesh | first divergence | meaning |
|---|---|---|
| tetrahedron | byte 40 (was 23) | per-attribute-encoder metadata (fine) |
| sphere | byte 15 | `num_attribute_data` — INPUT parity (GENERIC attrs) |
| bunny / torus | byte 11 | edgebreaker traversal type (STANDARD vs VALENCE) |

## The four remaining gaps, in priority order

### A. Valence Edgebreaker traversal — HIGHEST VALUE (fixes byte-identity AND size)

- **What:** Google selects `MESH_EDGEBREAKER_VALENCE_ENCODING` (byte 11 = `0x02`)
  for meshes with `num_faces >= 1000` at speed = `10 - cl` = 3
  (`mesh_edgebreaker_encoder.cc:25-67`). draco-oxide hardcodes STANDARD (`0x00`,
  `edgebreaker.rs:509`), and `Config::default` pins
  `traversal: EdgebreakerKind::Standard` (`edgebreaker.rs:91`).
- **Why it matters most:** it blocks byte-identity for EVERY real (large) mesh,
  and the valence connectivity is more compact — it is the bulk of the residual
  ~13% bunny size gap (oxide 78,507 B vs Google 69,169 B).
- **Effort:** large. A `ValenceTraversal` exists (`edgebreaker.rs:739`) but its
  `encode` (`:881`) is incomplete — no start-face-config and no attribute-seam
  encoding (comments at `:890`). Need: (1) method selection from speed/num_faces
  mirroring `InitializeEncoder`; (2) write `self.config.traversal` not the literal
  Standard; (3) plumb a speed/cl option into `Config` (currently unused, `:78`);
  (4) finish `ValenceTraversal::encode` (per-context valence symbol arrays,
  start-face replay, attribute seams) to byte-match Google's
  `mesh_edgebreaker_traversal_valence_encoder`.
- **Recommendation:** this is a real feature addition — worth a dedicated branch,
  an implementation plan, and review. Verify with `bytediff bunny 11` (expect
  byte 11 to match, then chase the valence symbol stream) + interop + round-trip.

### B. GENERIC attributes from OBJ materials / object-group names — INPUT parity

- **What:** Google's `obj_decoder.cc` synthesizes GENERIC attributes from
  `mtllib`/`usemtl` (`:183-194`) and `o`/`g` names (`:218-228`). draco-oxide's
  loader (`io/obj/mod.rs`) ignores them, so sphere has 1 non-position attribute
  vs Google's 3 → `num_attribute_data` differs at byte 15, and everything after.
- **Effort:** medium (loader feature). Not an encoder bug.
- **Recommendation:** for clean conformance now, compare on OBJs without
  `mtllib`/`o`/`g`. Implement the GENERIC synthesis when material round-trip is
  needed. (Note: this only affects OBJ input; the glTF/b3dm path that tileforge
  actually uses carries its own attribute set.)

### C. Per-attribute-encoder metadata — fine divergences (tetra byte 40)

- **DECODED:** byte 40 is the **element_type** byte of the 3rd attribute
  encoder's identifier (the NORMAL attribute). Google writes
  `GetAttributeElementType(att_id)` — `MESH_VERTEX_ATTRIBUTE=0` or
  `MESH_CORNER_ATTRIBUTE=1`, chosen per attribute by seam analysis
  (`mesh_edgebreaker_encoder_impl.cc:240-256`): tetra's TEXCOORD is seam-split →
  `1` (matches at byte 37), but NORMAL is NOT seam-split → `0`. draco-oxide
  writes `att.get_domain()` instead (`encode/attribute/mod.rs:36`), which yields
  `1` for the normal → divergence (`0x00` google vs `0x01` oxide).
- **Fix:** in `encode/attribute/mod.rs:31-39`, emit the Draco element_type
  (0=vertex / 1=corner) derived from whether the attribute actually has its own
  attribute corner table (seams), NOT from `AttributeDomain`. Must coordinate
  with the DECODER, which reads this byte (`decode/attribute/mod.rs` dispatcher
  via `meta.decoder_id` / element-type) — verify round-trip + interop after.
- **Effort:** small fix, but touches the encoder/decoder contract and the seam
  classification; verify carefully. A long tail of similar fine divergences
  likely follows (chase with `bytediff tetrahedron 11`).

### D. Prediction / transform parity (from the earlier audit, files 01–02)

- Normal octahedral transform (float + hardcoded 8-bit vs Google's integer
  toolbox), parallelogram/texcoord fallback value, constrained
  multi-parallelogram (Google's default position predictor at speed 0–1). These
  affect the symbol VALUES and thus bytes; surface after A–C.

## Suggested sequence

1. (B) Use material-free OBJs (or add GENERIC synthesis) so sphere is comparable.
2. (A) Valence traversal — the big one; own branch + review. Biggest single win.
3. (C) Walk tetra byte-by-byte to byte-identical (validates the attribute path).
4. (D) Prediction parity for the symbol values.

Each step: verify with `bytediff`, the `conformance` interop + size gates, and
the draco-oxide round-trip suite (run `--release`; the debug suite is slow on
bunny's O(n²) `diff_l2_norm`).
