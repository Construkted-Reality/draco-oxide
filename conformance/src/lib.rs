//! Conformance & performance harness: **draco-oxide vs Google C++ Draco**.
//!
//! Goal of the Construkted fork (see `docs/audit/2026-06-17-google-parity/`):
//! a Google-**interoperable**, **size-competitive**, and **perf-competitive**
//! mesh + point-cloud codec. This crate is the test infrastructure that holds
//! those three gates honest with real numbers, not code-reading.
//!
//! ## The three gates
//! 1. **Interop** — Google's tools must decode draco-oxide's output, and
//!    draco-oxide must decode Google's output. (`tests/interop.rs`)
//! 2. **Size-parity** — `oxide_bytes / google_bytes` must stay under
//!    [`SIZE_SHIP_CEILING`]; goal is [`SIZE_GOAL`]. (`tests/size_parity.rs`)
//! 3. **Perf-parity** — encode/decode wall-time within the perf ceiling of
//!    Google, native release builds. (`benches/codec.rs`, `tests/perf.rs`)
//!
//! ## Reference mechanism
//! We shell out to the installed Google CLI binaries `draco_encoder` /
//! `draco_decoder` (override with env `DRACO_ENCODER` / `DRACO_DECODER`).
//! This is deliberate: the in-process `draco_decoder` cxx crate (0.0.26)
//! currently fails to link (`rust-lld: error: unable to find library -ldraco`
//! — its build script emits a relative `-L` path), so `draco-oxide`'s own
//! `google_compat.rs` can't run anywhere. The CLI route needs no build and
//! matches the interop bar directly. In-process C++ timing (for the tightest
//! perf comparison) is deferred until the cxx link is fixed.
//!
//! Corpus today: position-dominant mesh fixtures under
//! `draco-oxide/tests/data/`. Point-cloud fixtures (from the reference
//! `testdata/`) get wired in once the Rust point-cloud path exists.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use draco_oxide::decode::{self, decode};
use draco_oxide::encode::{self, encode};
use draco_oxide::io::obj::load_obj;
use draco_oxide::prelude::{AttributeType, ConfigType, Mesh};

/// Hard ceiling for the size gate: encoded size must not exceed this multiple
/// of Google's for the same input. Anything above this blocks the gate.
pub const SIZE_SHIP_CEILING: f64 = 2.0;
/// Aspirational size-parity goal (parity with Google).
pub const SIZE_GOAL: f64 = 1.2;
/// Hard ceiling for the perf gate (native release, per operation).
pub const PERF_SHIP_CEILING: f64 = 2.0;

/// Google `draco_encoder` binary (env `DRACO_ENCODER`, else PATH).
pub fn google_encoder_bin() -> String {
    std::env::var("DRACO_ENCODER").unwrap_or_else(|_| "draco_encoder".to_string())
}

/// Google `draco_decoder` binary (env `DRACO_DECODER`, else PATH).
pub fn google_decoder_bin() -> String {
    std::env::var("DRACO_DECODER").unwrap_or_else(|_| "draco_decoder".to_string())
}

/// Workspace root (the directory that contains the `draco-oxide/` crate).
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("conformance/ has a parent")
        .to_path_buf()
}

/// Directory holding the OBJ mesh fixtures.
pub fn mesh_dir() -> PathBuf {
    workspace_root().join("draco-oxide/tests/data")
}

fn unique_tmp(ext: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("draco_conf_{}_{}.{}", std::process::id(), n, ext))
}

/// Decode `.drc` bytes with draco-oxide. Panics are caught and surfaced as
/// `Err`. (draco-oxide may not decode every Google stream yet — see the audit.)
pub fn oxide_decode(drc: &[u8]) -> Result<Mesh, String> {
    let drc = drc.to_vec();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut reader = drc.into_iter();
        decode(&mut reader, decode::Config::default()).map_err(|e| format!("decode: {e:?}"))
    }))
    .unwrap_or_else(|_| Err("panic during oxide decode".to_string()))
}

/// Standard mesh corpus for the harness (position-dominant fixtures that the
/// existing round-trip / google-compat tests already exercise).
pub fn mesh_corpus() -> Vec<&'static str> {
    vec!["tetrahedron.obj", "sphere.obj", "torus.obj", "bunny.obj"]
}

/// Run Google's encoder on an OBJ and return the `.drc` bytes.
///
/// `position_only` passes `--skip NORMAL/TEX_COORD/GENERIC` to isolate the
/// position attribute (where the size-blowup lives); `false` encodes the full
/// mesh for an apples-to-apples total-size comparison.
pub fn google_encode(obj: &Path, qp: u32, cl: u32, position_only: bool) -> Vec<u8> {
    let out = unique_tmp("drc");
    let mut cmd = Command::new(google_encoder_bin());
    cmd.arg("-i")
        .arg(obj)
        .arg("-o")
        .arg(&out)
        .arg("-qp")
        .arg(qp.to_string())
        .arg("-cl")
        .arg(cl.to_string());
    if position_only {
        cmd.args(["--skip", "NORMAL", "--skip", "TEX_COORD", "--skip", "GENERIC"]);
    }
    let status = cmd
        .status()
        .expect("run draco_encoder (is it on PATH, or set DRACO_ENCODER?)");
    assert!(status.success(), "draco_encoder failed on {obj:?}");
    let bytes = std::fs::read(&out).expect("read google .drc");
    let _ = std::fs::remove_file(&out);
    bytes
}

/// Parse a "`<n> ms to <verb>`" timing the Google CLI prints to stdout (encode
/// or decode time, excluding process startup + file IO). `None` if not present.
pub fn parse_cli_ms(stdout: &str, verb: &str) -> Option<f64> {
    let marker = format!(" ms to {verb}");
    let i = stdout.find(&marker)?;
    let num: String = stdout[..i]
        .chars()
        .rev()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.chars().rev().collect::<String>().parse().ok()
}

/// Like [`google_encode`] but also returns Google's self-reported encode time
/// in milliseconds (codec-only, no IO), parsed from the CLI output.
pub fn google_encode_timed(obj: &Path, qp: u32, cl: u32, position_only: bool) -> (Vec<u8>, Option<f64>) {
    let out = unique_tmp("drc");
    let mut cmd = Command::new(google_encoder_bin());
    cmd.arg("-i").arg(obj).arg("-o").arg(&out)
        .arg("-qp").arg(qp.to_string())
        .arg("-cl").arg(cl.to_string());
    if position_only {
        cmd.args(["--skip", "NORMAL", "--skip", "TEX_COORD", "--skip", "GENERIC"]);
    }
    let output = cmd.output().expect("run draco_encoder");
    assert!(output.status.success(), "draco_encoder failed on {obj:?}");
    let ms = parse_cli_ms(&String::from_utf8_lossy(&output.stdout), "encode");
    let bytes = std::fs::read(&out).expect("read google .drc");
    let _ = std::fs::remove_file(&out);
    (bytes, ms)
}

/// Decode `.drc` bytes with Google's decoder, returning Google's self-reported
/// decode time in milliseconds (codec-only) alongside the result.
pub fn google_decode_timed(drc: &[u8]) -> (Result<String, String>, Option<f64>) {
    let in_path = unique_tmp("drc");
    let out_path = unique_tmp("obj");
    std::fs::write(&in_path, drc).expect("write tmp .drc");
    let output = Command::new(google_decoder_bin())
        .arg("-i").arg(&in_path).arg("-o").arg(&out_path)
        .output()
        .expect("run draco_decoder");
    let _ = std::fs::remove_file(&in_path);
    let ms = parse_cli_ms(&String::from_utf8_lossy(&output.stdout), "decode");
    if !output.status.success() {
        let _ = std::fs::remove_file(&out_path);
        return (Err(String::from_utf8_lossy(&output.stderr).into_owned()), ms);
    }
    let obj = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    (Ok(obj), ms)
}

/// Decode `.drc` bytes with Google's decoder. `Ok(obj_text)` if Google could
/// decode them (the interop check), `Err(stderr)` otherwise.
pub fn google_decode(drc: &[u8]) -> Result<String, String> {
    let in_path = unique_tmp("drc");
    let out_path = unique_tmp("obj");
    std::fs::write(&in_path, drc).expect("write tmp .drc");
    let output = Command::new(google_decoder_bin())
        .arg("-i")
        .arg(&in_path)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("run draco_decoder (is it on PATH, or set DRACO_DECODER?)");
    let _ = std::fs::remove_file(&in_path);
    if !output.status.success() {
        let _ = std::fs::remove_file(&out_path);
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let obj = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    Ok(obj)
}

/// Encode an OBJ with draco-oxide at the given position quantization bits.
///
/// The audit found draco-oxide panics on some meshes (e.g. unused vertices),
/// so panics are caught and surfaced as `Err` rather than aborting the whole
/// test binary.
pub fn oxide_encode(obj: &Path, position_bits: u8) -> Result<Vec<u8>, String> {
    let mesh = load_obj(obj).map_err(|e| format!("load_obj: {e:?}"))?;
    oxide_encode_mesh(mesh, position_bits)
}

/// Load an OBJ into a `Mesh` (for benches that want to exclude parse time).
pub fn load_mesh(obj: &Path) -> Mesh {
    load_obj(obj).expect("load_obj")
}

/// Encode an already-loaded mesh. `encode` consumes the mesh, so benches clone
/// a pre-parsed mesh per iteration to time encode (plus a cheap clone) without
/// the OBJ parse. Panics are caught and surfaced as `Err`.
pub fn oxide_encode_mesh(mesh: Mesh, position_bits: u8) -> Result<Vec<u8>, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut cfg = encode::Config::default();
        cfg.set_attribute_quantization_bits(AttributeType::Position, position_bits);
        let mut buf = Vec::new();
        encode(mesh, &mut buf, cfg).map_err(|e| format!("encode: {e:?}"))?;
        Ok(buf)
    }))
    .unwrap_or_else(|_| Err("panic during oxide encode".to_string()))
}
