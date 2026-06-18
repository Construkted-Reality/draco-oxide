# conformance — draco-oxide ↔ Google C++ Draco harness

Test infrastructure that holds the Construkted fork's three goals honest with
**real numbers**, not code-reading. Target codec scope: Google-interoperable,
size-competitive, perf-competitive **mesh + point-cloud** compression (see
`../docs/audit/2026-06-17-google-parity/`).

## The three gates

| Gate | What it asserts | File |
|------|-----------------|------|
| **Interop** | Google's tools decode draco-oxide output (and vice-versa) | `tests/interop.rs` |
| **Size-parity** | `oxide_bytes / google_bytes ≤ 2.0` (goal 1.2) | `tests/size_parity.rs` |
| **Perf-parity** | encode/decode wall-time ≤ 2.0× Google, native release | `benches/codec.rs`, `tests/perf.rs` |

The size and perf **ship ceilings are `2.0×`**; the **goal is `1.2×`** (parity).
`5×` is the hard failure line. Some gates are expected RED today — they encode
the known bugs from the audit as failing tests that go green when fixed (e.g.
the qp14 size-blowup).

## Reference mechanism

Shells out to the installed Google CLI binaries `draco_encoder` / `draco_decoder`
(override with env `DRACO_ENCODER` / `DRACO_DECODER`). We deliberately avoid the
in-process `draco_decoder` cxx crate (0.0.26): it currently fails to link
(`rust-lld: error: unable to find library -ldraco` — its build script emits a
relative `-L` path), which also blocks draco-oxide's own `google_compat.rs`.
In-process C++ timing for the tightest perf comparison is deferred until that
link is fixed.

## Running it (memory-safe on a shared box)

This machine runs other workloads. Always build/run inside a memory-capped
cgroup so an overrun kills only this build, never the rest of the system:

```bash
# tests (debug) — never use a bare `cargo test` (workspace-wide) because that
# pulls draco-oxide's draco_decoder dev-dep and compiles all of libdraco C++.
systemd-run --user --scope -q -p MemoryMax=4G -p MemorySwapMax=1G nice -n15 \
  cargo test -p conformance -j2 -- --nocapture

# benches (release)
systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=1G nice -n15 \
  cargo bench -p conformance -j2
```

`-- --nocapture` shows the ratio/perf tables even when the gate passes.

## Corpus

- **Meshes:** position-dominant OBJ fixtures in `../draco-oxide/tests/data/`
  (`mesh_corpus()` in `src/lib.rs`).
- **Point clouds:** wired in once the Rust point-cloud path exists; reference
  `.ply` fixtures live in `../google-draco-reference/testdata/`.

## Roadmap

1. Fix the `draco_decoder` cxx link → in-process C++ decode + accurate perf.
2. Add the point-cloud corpus + gates as the Rust PC path comes online.
3. Tighten size/perf tolerances from `2.0×` toward the `1.2×` goal as fixes land.
