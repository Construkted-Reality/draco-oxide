# Implementation plan — Valence Edgebreaker traversal (encoder)

**Goal:** make draco-oxide emit Google's VALENCE Edgebreaker connectivity when
Google would, byte-identically. Fixes the byte-11 divergence on all meshes
≥1000 faces and the bulk of the ~13% bunny size gap (valence connectivity is
more compact). Decode-perf may also improve.

## Why this is smaller than it looks (key enablers)

1. **The decoder already supports valence.** `decode/connectivity/edgebreaker.rs:94-119`
   reads start-faces + attribute-seams, then 6 per-context symbol arrays
   (valence 2..=7), mirroring `mesh_edgebreaker_traversal_valence_decoder.h`. So
   we only need the ENCODER path; round-trip is already wired.
2. **`ValenceTraversal::record_symbol` is already implemented** (`encode/connectivity/edgebreaker.rs:777-867`)
   and matches Google's algorithm (`mesh_edgebreaker_traversal_valence_encoder.h:75-185`):
   per-vertex valence tracking, the C/S/R/L/E valence decrements, the S-split
   left/right fan walk with `corner_to_vertex_map` remap + new vertex, the
   `active_valence` captured before the decrements, and the one-symbol-lag
   context emit (`prev_symbol` keyed by the current active context; first symbol
   emits nothing). This is the hard part and it's done.
3. **Start-faces and attribute-seams already byte-match Google** in the standard
   path (`DefaultTraversal::encode:670-733`) — tetra's STANDARD connectivity is
   byte-identical to Google through byte 22, which includes both. We reuse this
   code verbatim for valence.

## What's missing / wrong (the work)

`ValenceTraversal::encode` (`edgebreaker.rs:881-908`) is incomplete:
- start-faces + seams are commented out (`:890-891`);
- it ignores `att_data` and `corner_table` (`:884-885`);
- `ValenceTraversal::new_corner_reached` (`:873-875`) only sets `last_corner`, so
  it never accumulates `processed_connectivity_corners` (needed for seams);
- contexts are forced `DirectCoded` (`:904`) — Google uses the ADAPTIVE
  `EncodeSymbols` (num_components=1, options=nullptr) per context.

And the dispatch always emits STANDARD:
- `encode_connectivity:509` hardcodes `EdgebreakerKind::Standard.write_to(writer)`;
- the traversal type `T` is a compile-time generic with no runtime selection;
- `Config` has no speed/compression-level field to drive the choice
  (`Config::default` pins `traversal: Standard`, `edgebreaker.rs:91`; `config` is
  otherwise unused, `:78`).

## Work items

### W1 — Method selection + Config plumbing
- Add a speed/compression-level to `encode::Config` (Google: `speed = 10 - cl`,
  default cl 7 → speed 3). Mirror `InitializeEncoder`
  (`mesh_edgebreaker_encoder.cc:34-45`): pick STANDARD if `num_faces < 1000 ||
  speed >= 5`, else VALENCE. (Predictive is out of scope.)
- Write the selected method byte (`0x00` STANDARD / `0x02` VALENCE,
  `compression_shared.h:122-126`) at `encode_connectivity:509` instead of the
  literal Standard.

### W2 — Runtime traversal dispatch
- The encoder is generic over `T: Traversal`. Select at the call site
  (`encode/connectivity/mod.rs` or wherever `encode_connectivity` is invoked):
  compute the method from `num_faces`+speed, then call the `DefaultTraversal` or
  `ValenceTraversal` monomorphization. (Simplest: a `match` that calls the right
  generic instantiation; avoid `dyn`.) Confirm there is exactly one construction
  site to branch.

### W3 — Complete `ValenceTraversal::encode`
- `new_corner_reached`: also push the corner into a `processed_connectivity_corners`
  Vec (like `DefaultTraversal:634-636`), so seams can be computed.
- Refactor the start-faces and attribute-seams blocks out of
  `DefaultTraversal::encode` into shared free functions
  `write_start_faces(writer, &interior_cfg)` and
  `write_attribute_seams(writer, &processed_corners, att_data, corner_table)`
  (byte-for-byte the current code).
- `ValenceTraversal::encode` writes, IN THIS ORDER (per spec §c, valence `Done()`
  `..._valence_encoder.h:187-201`): **start-faces → attribute-seams → 6 context
  arrays**. (NB: standard order is symbols → start-faces → seams; valence has NO
  inline symbol block and the contexts come LAST. Keep the helpers, change the
  order.)
- Each context: `leb128(count)` then, if count>0, `encode_symbols(symbols, 1,
  None, writer)` — **None = adaptive selection** to match Google. Emit all 6
  contexts (valence 2..7) including empty ones (count 0).

### W4 — Verify context scheme choice
- Google encodes each context with adaptive `EncodeSymbols`. Max symbol value ≤4
  (5 topology ids), so raw is likely chosen, but confirm per-context against the
  instrumented Google encoder; using `None` makes oxide match whatever Google
  picks. (If a mismatch appears, it is the estimator, already ported.)

### W5 — (Related; needed for full byte-identity, NOT blocking bunny)
- `encode_connectivity:514-515` writes raw `num_vertices()` / `faces.len()`;
  Google writes `num_vertices - NumIsolatedVertices` and `num_faces -
  NumDegeneratedFaces` (`mesh_edgebreaker_encoder_impl.cc:295-301`). Bunny/torus
  have neither, so this does not block them — track separately, but do it for
  correctness on dirty meshes.

## Verification

1. `bytediff bunny 11` — expect byte 11 to flip to `0x02` and connectivity to
   match much further (chase any per-context divergence).
2. `bytediff torus 11` (valence, 4095 faces) and confirm `tetrahedron`/`sphere`
   (<1000 faces) STAY on STANDARD and unchanged.
3. conformance interop gate (Google decodes oxide valence output) + self-roundtrip
   (oxide decodes its own valence output — exercises the existing valence decoder).
4. Size: expect bunny oxide to drop from 78,507 B toward Google's 69,169 B.
5. Regenerate `encode_byte_stability` fingerprints (bunny/torus change).
6. Run draco-oxide suite `--release` + conformance, all green.

## Risks / watch-items

- **Two-buffer field-order trap** (spec §e): `num_encoded_symbols`,
  `num_split_symbols`, split data live in the main buffer BEFORE the traversal
  data; start-faces/seams/contexts are the traversal data. oxide already writes
  num_symbols/num_split/split before `traversal.encode()` (`:565-573`), so the
  order is right — but `num_symbols` for valence = `record_symbol` count, and the
  decoder invariant is `sum(context counts) == num_symbols - 1` (last symbol
  never emitted). Verify oxide's `num_symbols` semantics match.
- **`num_split_symbols` / topology splits**: valence and standard share
  `encode_topology_splits`; confirm it's traversal-independent (it operates on
  the corner table, should be).
- **Refactor blast radius**: extracting start-faces/seams helpers touches the
  working standard path — must stay byte-identical (tetra fingerprint must not
  change). Verify standard output is unchanged after the refactor BEFORE wiring
  valence.
- **Config change** ripples to all `Config::default()` callers and the
  byte-stability/oracle tests (they use default cl=7 → speed 3 → bunny/torus
  become VALENCE). Expected; regenerate.

## Review corrections (validated by two independent reviewers — these SUPERSEDE the above)

1. **CONFIRMED BUG in `record_symbol` (was claimed "done").** oxide's `Symbol::C`
   arm is EMPTY (`edgebreaker.rs:791`), but Google's `TOPOLOGY_C` falls through to
   `TOPOLOGY_S` and runs `vertex_valences_[next] -= 1; vertex_valences_[prev] -= 1`
   (NOT the split block) — `mesh_edgebreaker_traversal_valence_encoder.h:91-97`.
   **Fix:** make the C arm decrement next/prev by 1 (share with S's first two
   lines). Without this, valence diverges from Google AND from oxide's own
   decoder (round-trip would fail). Everything else in `record_symbol` verified
   correct.

2. **W2 was misdiagnosed — runtime dispatch ALREADY EXISTS.** `encode/connectivity/mod.rs:53-65`
   already `match`es `cfg.traversal` to the `DefaultTraversal` / `ValenceTraversal`
   monomorphization (both compile/unify; no `dyn`). The ACTUAL plumbing bugs are:
   - `encode/connectivity/mod.rs:31` calls the inner fn with `Config::default()`,
     **discarding the real `cfg`** — so `cfg.traversal` never reaches the match.
     Must thread the caller's `cfg` (and set `cfg.traversal` from num_faces+speed).
   - `edgebreaker.rs:509` hardcodes `EdgebreakerKind::Standard.write_to(writer)`
     regardless of `T` — must write `self.config.traversal` (the encoder already
     stores it). **This is the actual byte-11 bug.**
   So W1+W2 collapse to: add speed/cl to Config, compute the method, thread cfg
   through `mod.rs:31`, write the selected kind at `:509`. No new dispatch code.

3. **Selection rule:** add the `!predictive_available` term
   (`mesh_edgebreaker_encoder.cc:39-45`): STANDARD iff `speed>=5 ||
   !predictive_available || num_faces<1000`. EMPIRICALLY the installed Google
   build picks VALENCE for bunny (bytediff byte 11 = `0x02`), so predictive IS
   available in our reference — the bunny→valence premise holds.

4. **W4 is a BLOCKING gate, not a footnote.** Contexts must use
   `encode_symbols(.., None, ..)` (adaptive) to match Google. Verify byte-level
   that `None`'s emitted scheme tag is what the decoder's `decode_symbols(n,1)`
   reads — both reviewers flagged this as the one real interop risk.

5. **Fixtures to regenerate:** `EXPECT_TORUS_*` and `EXPECT_BUNNY_*` in
   `encode_byte_stability.rs` (≥1000 faces → flip to valence at cl7/speed3).
   `EXPECT_TETRA_*`/`EXPECT_SPHERE_*` STAY standard (<1000 faces) — keep as a
   regression assertion. The `sequence` oracle is `T`-independent
   (`corners_of_edgebreaker` is unchanged by traversal) → no change. `round_trip`
   / `google_compat` assert geometry not bytes → re-run as interop gates.

6. **No compile blockers; decoder fully compatible** (verified: reads
   start-faces+seams then 6 leb128-counted adaptive context arrays, replay forces
   first symbol = E, consumes `num_symbols-1` context entries). `corners_of_edgebreaker`,
   `encode_topology_splits`, `num_split_symbols` are all traversal-independent.

## Suggested commit sequence (own branch off feat/google-parity)

1. Extract start-faces/seams helpers; prove standard output byte-unchanged.
2. Add Config speed + method selection + conditional method byte (still always
   Standard at speed-from-cl7? No — cl7→speed3→valence for big meshes, so this
   step flips bunny; do it with W2/W3 together).
3. Runtime dispatch + complete `ValenceTraversal::encode`.
4. Verify (bytediff/interop/roundtrip/size), regenerate fingerprints.
