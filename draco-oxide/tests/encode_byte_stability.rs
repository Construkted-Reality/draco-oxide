//! End-to-end encode-output stability gate.
//!
//! Locks the encoded byte stream for a set of meshes so the O(n^2) perf fixes
//! in `compute_sequence` (Edgebreaker traversal order) and
//! `MeshParallelogramPrediction::predict` (attribute prediction) cannot
//! silently change output. draco-oxide's encoded bytes are consumed by
//! Google's Draco decoder in the field, so they must stay byte-identical
//! across these optimizations.
//!
//! The fingerprints were captured from the pre-optimization implementation and
//! confirmed byte-identical on the full 930-tile HighPoly corpus.

use draco_oxide::prelude::ConfigType;
use draco_oxide::{
    encode::{self, encode},
    io::obj::load_obj,
};

/// FNV-1a over the encoded byte stream. Deterministic, dependency-free.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn encode_fingerprint(obj: &str) -> (usize, u64) {
    let mesh = load_obj(obj).unwrap();
    let mut buf = Vec::new();
    encode(mesh, &mut buf, encode::Config::default()).unwrap();
    (buf.len(), fnv1a(&buf))
}

#[test]
fn encode_output_is_byte_stable() {
    // (obj, expected_len, expected_fnv1a). tetrahedron carries position +
    // normal + texcoord attributes, exercising all three mesh prediction
    // schemes; sphere/torus/bunny exercise position (parallelogram) at scale
    // and over handle topology (torus).
    let cases: &[(&str, usize, u64)] = &[
        (
            "tests/data/tetrahedron.obj",
            EXPECT_TETRA_LEN,
            EXPECT_TETRA_HASH,
        ),
        (
            "tests/data/sphere.obj",
            EXPECT_SPHERE_LEN,
            EXPECT_SPHERE_HASH,
        ),
        ("tests/data/torus.obj", EXPECT_TORUS_LEN, EXPECT_TORUS_HASH),
        ("tests/data/bunny.obj", EXPECT_BUNNY_LEN, EXPECT_BUNNY_HASH),
        // Exercises the non-manifold path (edge shared by 3 faces ->
        // handle_no_manifold_edges), which the other fixtures never trigger.
        (
            "tests/data/nonmanifold_edge.obj",
            EXPECT_NM_LEN,
            EXPECT_NM_HASH,
        ),
    ];
    let dump = std::env::var("DUMP_ENCODE_FINGERPRINTS").is_ok();
    for (obj, exp_len, exp_hash) in cases {
        let (len, hash) = encode_fingerprint(obj);
        if dump {
            eprintln!("{obj} => len={len} hash={hash}");
            continue;
        }
        assert_eq!(
            (len, hash),
            (*exp_len, *exp_hash),
            "encoded output changed for {obj}"
        );
    }
}

// Regenerated 2026-06-17 after the adaptive symbol-scheme-selection + rANS
// frequency-table run-length fix (encode/entropy/{symbol_coding,rans}.rs).
// Output legitimately shrank (e.g. tetra 846->191, sphere 1962->587) and now
// matches Google's scheme choices. Regenerate with
// `DUMP_ENCODE_FINGERPRINTS=1 cargo test -p draco-oxide --test encode_byte_stability -- --nocapture`.
// Regenerated 2026-06-18 after the Google-byte-identical NORMAL attribute work:
// integer octahedral ComputeCorrection/ComputeOriginalValue + i64 face-normal
// cross product, plus aligning position quantization to Google's
// `Quantizer` (precomputed f32 `inverse_delta` then floor(v+0.5)), which
// removed off-by-one quantized positions. bunny now matches Google's encoded
// size exactly (69169 B) and its NORMAL correction symbols + flip bits are
// byte-identical to Google's; the only residual byte diffs are in the entropy
// (rANS / flip-RABS) layer.
// Regenerated 2026-06-18 after encoding the per-attribute element type the way
// Google does: a corner attribute with no interior (non-boundary) seams is
// written as MESH_VERTEX_ATTRIBUTE (0) rather than MESH_CORNER_ATTRIBUTE (1).
// This made bunny byte-IDENTICAL to Google (the last diverging byte) and shifted
// the same descriptor byte for the NORMAL/COLOR attributes of tetra/sphere; only
// those three fingerprints changed (torus is position-only, unaffected).
const EXPECT_TETRA_LEN: usize = 188;
const EXPECT_TETRA_HASH: u64 = 867813181395307981;
const EXPECT_SPHERE_LEN: usize = 613;
const EXPECT_SPHERE_HASH: u64 = 6661504626970154205;
// torus and bunny (>= 1000 faces) now use VALENCE Edgebreaker at cl7/speed3,
// matching Google; their connectivity is byte-identical to Google and the
// output shrank (torus 3414->2490 == Google, bunny 78507->66567).
const EXPECT_TORUS_LEN: usize = 2490;
const EXPECT_TORUS_HASH: u64 = 6189417996939192234;
const EXPECT_BUNNY_LEN: usize = 69169;
const EXPECT_BUNNY_HASH: u64 = 2710872942979440892;
// Tiny non-manifold fixture (edge shared by 3 faces). Byte-identical to Google.
const EXPECT_NM_LEN: usize = 89;
const EXPECT_NM_HASH: u64 = 4728961036571408597;
