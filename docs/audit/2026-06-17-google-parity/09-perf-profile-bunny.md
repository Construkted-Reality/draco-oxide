# Perf profile — bunny encode/decode (Rust vs C++)

**Date:** 2026-06-18. Release + `debuginfo=2`, `perf record --call-graph dwarf -F 2000`,
bunny.obj @ qp11/cl7, valence connectivity. Driver: `conformance` bin `profile`.

## Wall-time gap being explained (min-of-50, in-process, codec-only)

| | Rust | C++ | ratio |
|---|---:|---:|---:|
| encode bunny | 42.2 ms | 23.7 ms | **1.8×** |
| decode bunny | 21.8 ms | 13.2 ms | **1.6×** |

## ENCODE — where the 42 ms goes (perf self-time %)

| area | % | functions |
|---|---:|---|
| **Corner-table construction** | **~35%** | `CornerTable::new` 16.5, `AttributeCornerTable::new` 7.4, sorting (`quicksort` 6.4 + smallsort 4.2 + sort4 0.9) |
| Corner-table navigation | ~11% | RefAttributeCornerTable `opposite` 5.2, `vertex_idx` 4.0, `next` 2.0 |
| Prediction | ~12% | normal `compute_normal_of_face` 4.3 + `predict` 3.6, parallelogram `predict` 3.5 |
| Traversal / valence | ~10% | `edgebreaker_from` 4.8, `record_symbol` 2.6, `ValenceTraversal::new` 2.3, `compute_sequence` 3.5 |
| Entropy | ~5% | `encode_symbols_direct_coded` 2.3, … |
| Hashing / memmove | ~8% | `HashMap::insert` 2.8, `hash_one` 1.8, `memmove` 3.1 |

**The dominant cost is corner-table construction (~35%).** oxide builds the
opposite-corner map by **sorting edges (O(n log n))** — that's the quicksort/
smallsort time. Google builds it with an **edge hash map (O(n))**. This is the
single biggest encode lever and also helps decode.

## DECODE — where the 21.8 ms goes

| area | % | functions |
|---|---:|---|
| **Output mesh assembly** | **25.8%** | inlined into `decode::decode`: sort + `HashMap<CornerTuple,u32>` dedup of per-corner attribute tuples into output vertices (`decode/mod.rs:313-372`), over bunny's ~208k corners |
| Normal prediction | 12.7% | `predict_normal` |
| Corner-table / attr-table build | ~18% | `DecoderAttributeCornerTable::build_with_offsets` 9.8, `recompute_left_most_corners` 5.8, `build_attribute_corner_tables` 2.5 |
| `compute_sequence` | ~10% | 6.9 + 3.3 (per attribute) |
| Edgebreaker replay | 7.8% | `replay_symbols` |
| Entropy | 5.4% | `decode_symbols_direcd_coded` |

The `decode::decode` 25.8% is a tight scan/dedup loop (asm: `incq`/`je`/`sbbb`
conditional-count) — the per-corner-tuple dedup + sorted-id ranking that turns
Draco's corner attributes into a unified indexed mesh.

## Optimization targets, ranked by impact × tractability

1. **Hash-based corner-table construction** (encode ~35%, decode ~18%). Replace
   the sort-based opposite-corner finding with an edge hash map (Google's
   approach). Biggest single lever; helps both directions. Output is unchanged
   (pure internal build), so it can't break byte-identity.
2. **Decode output-mesh assembly** (25.8%). The sort + `std::HashMap` dedup over
   208k corners. Swap to a faster hasher (FxHash/ahash) and/or drop the
   sort-based ranking for a direct hash remap. Output mesh unchanged.
3. **Normal prediction** (encode ~8%, decode 12.7%). The per-face normal compute
   + octahedral transform. The audit already flags this path as float-based and
   non-Google-faithful; an integer/optimized rewrite would help perf AND
   byte-identity together.
4. **`compute_sequence`** (decode ~10%, encode ~3.5%). Runs per attribute; check
   for redundant calls and O(n) tightening.

## Results (items 1–2 implemented, output byte-identical)

| bunny, vs C++ | before | after |
|---|---:|---:|
| encode | 1.8× (42 ms) | **1.5–1.6× (~37 ms)** |
| decode | 1.6× (22 ms) | **1.1× (15 ms)** |

What actually moved the needle:
- **encode**: the `contains_non_manifold_edges` O(E log E) sort + per-face Vec
  allocs → O(E) FxHashMap count. ~4 ms / ~10%.
- **decode**: the BTreeSet remap in `decode_with_warnings` (the *Mesh* path; the
  profiled path — I first mis-optimized `build_raw`, the flat-bytes path) → three
  linear passes. ~6 ms; decode is now essentially at parity.
- The decode dedup `HashMap` → `FxHashMap` swap (the literal "item 2") had
  negligible effect — the tuple hash was never the bottleneck.

Remaining headroom: encode is still ~1.5–1.6×, mostly the rest of corner-table
construction (`compute_table` bucketing + `AttributeCornerTable::new`, ~25%),
prediction (~12%), and navigation (~11%). Decode at 1.1× — `predict_normal`
(12.5%) and the decode corner-table build (~16%) are the next items but the
return is small now.

Notes:
- Items 1–2 are pure internal-data-structure work (no bitstream change) → safe to
  do without touching byte-identity. Item 3 changes attribute bytes → couple it
  with the normal byte-identity work (gap D).
- bunny carries normals, so the normal-prediction cost is real here; a
  position-only mesh would shift the mix toward corner-table + assembly.
- Profiling artifacts (`/tmp/*.perf`, ~350 MB each) were removed after analysis.
