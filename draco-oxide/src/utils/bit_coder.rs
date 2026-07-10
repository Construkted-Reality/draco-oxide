use crate::{
    core::bit_coder::ReaderErr,
    prelude::{ByteReader, ByteWriter},
};

#[allow(unused)]
pub(crate) fn leb128_read<W>(reader: &mut W) -> Result<u64, ReaderErr>
where
    W: ByteReader,
{
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = reader.read_u8()?;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    Ok(result)
}

/// Maximum bytes a single declared-length read pre-allocates up front.
/// A larger declared length still succeeds — the buffer grows on
/// demand — but the up-front allocation can't be larger than this,
/// so a hostile bitstream containing a multi-GB leb128 length can't
/// trigger an OOM-abort before any actual bytes are read. If the
/// stream truly does carry more than the cap, the per-byte reads
/// will succeed and the `Vec` will reallocate normally.
const PREALLOC_CAP: usize = 64 * 1024 * 1024;

/// Read `declared_len` bytes from `reader` into a fresh `Vec<u8>`.
/// Use this any time the length comes from the bitstream itself
/// (typically via `leb128_read`) — it caps the initial allocation so
/// that a malformed length field can't OOM-abort the process before
/// the decoder gets a chance to surface a `ReaderErr`.
pub(crate) fn read_byte_buffer<R>(reader: &mut R, declared_len: usize) -> Result<Vec<u8>, ReaderErr>
where
    R: ByteReader,
{
    let cap = declared_len.min(PREALLOC_CAP);
    let mut buf = Vec::with_capacity(cap);
    for _ in 0..declared_len {
        buf.push(reader.read_u8()?);
    }
    Ok(buf)
}

/// Largest number of *decoded elements* (faces, vertices, symbols, …) that
/// a single byte of compressed input can legitimately expand into.
///
/// Draco's most compact primitive — a 1-bit Edgebreaker `C` symbol —
/// encodes at best ~8 elements per input byte, and every real element also
/// drags along attribute/residual bytes, so legitimate meshes sit far below
/// this ratio. The cap is set two orders of magnitude above that ~8/byte
/// ceiling so every real mesh passes with huge margin, while a
/// decompression bomb — e.g. a ~40-byte blob declaring `num_faces = 1e9`
/// (~25 million elements/byte, which would drive a ~48 GB
/// `vec![NO_CORNER; num_faces*3]`) — is rejected by four-plus orders of
/// magnitude before the count-derived allocation ever runs.
///
/// This mirrors the `PREALLOC_CAP` discipline for byte-buffer preallocs:
/// an attacker-controlled count read mid-stream must not be allowed to
/// drive an allocation the compressed input could not possibly justify.
pub(crate) const MAX_ELEMENTS_PER_INPUT_BYTE: usize = 1024;

/// Upper bound on a bitstream-declared element count given `remaining`
/// input bytes still available in the reader, or `None` when the reader
/// can't report its remaining length (in which case the count can't be
/// bounded this way and callers should skip the check).
///
/// The `+1` admits a small count even when the declaring field sits at the
/// very end of the stream (zero bytes remaining after it is read).
pub(crate) fn max_admissible_count(remaining: Option<usize>) -> Option<usize> {
    remaining.map(|r| {
        r.saturating_add(1)
            .saturating_mul(MAX_ELEMENTS_PER_INPUT_BYTE)
    })
}

/// Returns `true` when `count` is too large to be legitimately produced by
/// `remaining` remaining input bytes — i.e. a suspected decompression bomb.
/// Always `false` when `remaining` is `None` (unbounded reader).
pub(crate) fn count_exceeds_remaining_input(count: usize, remaining: Option<usize>) -> bool {
    matches!(max_admissible_count(remaining), Some(max) if count > max)
}

pub(crate) fn leb128_write<W>(mut value: u64, writer: &mut W)
where
    W: ByteWriter,
{
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            writer.write_u8(byte);
            break;
        } else {
            writer.write_u8(byte | 0x80);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_byte_buffer_does_not_oom_on_huge_declared_length() {
        // Empty stream + a leb128-style declared length of "u32::MAX".
        // The cap means we don't try to allocate 4 GiB up front; the
        // per-byte reads then fail with `ReaderErr` at byte 0.
        let bytes: Vec<u8> = Vec::new();
        let mut reader = bytes.into_iter();
        let res = read_byte_buffer(&mut reader, u32::MAX as usize);
        assert!(
            res.is_err(),
            "should error rather than allocate huge buffer"
        );
    }

    #[test]
    fn read_byte_buffer_succeeds_when_bytes_available() {
        let bytes: Vec<u8> = (0u8..16).collect();
        let mut reader = bytes.into_iter();
        let buf = read_byte_buffer(&mut reader, 16).unwrap();
        assert_eq!(buf, (0u8..16).collect::<Vec<_>>());
    }

    #[test]
    fn count_bound_rejects_decompression_bomb() {
        // The headline attack: a ~40-byte blob declaring num_faces = 1e9,
        // which would drive a ~48 GB `vec![NO_CORNER; num_faces*3]`. With
        // only ~30 input bytes remaining the count is rejected — WITHOUT
        // ever performing the allocation (this is a pure arithmetic check).
        assert!(count_exceeds_remaining_input(1_000_000_000, Some(30)));
        // Even a moderate blow-up (100k elements from a near-empty tail) is
        // rejected: 0 remaining => max = 1 * 1024 = 1024.
        assert!(count_exceeds_remaining_input(100_000, Some(0)));
        assert!(count_exceeds_remaining_input(1025, Some(0)));
    }

    #[test]
    fn count_bound_admits_real_mesh_ratios() {
        // Real meshes carry far more than ~1/1024 byte per element, so
        // legitimate counts always clear the bound. A 70k-face mesh whose
        // compressed remainder is a few KB, and small meshes near the tail.
        assert!(!count_exceeds_remaining_input(70_000, Some(4_096)));
        assert!(!count_exceeds_remaining_input(4, Some(50)));
        // Exactly at the boundary is admitted; one past it is not.
        assert_eq!(max_admissible_count(Some(0)), Some(1024));
        assert!(!count_exceeds_remaining_input(1024, Some(0)));
        assert!(count_exceeds_remaining_input(1025, Some(0)));
    }

    #[test]
    fn count_bound_skipped_when_remaining_unknown() {
        // A streaming reader that can't report remaining length disables the
        // check (never a false-positive rejection); huge counts pass here and
        // are instead caught by per-element read-exhaustion.
        assert!(!count_exceeds_remaining_input(usize::MAX, None));
        assert_eq!(max_admissible_count(None), None);
    }

    #[test]
    fn count_bound_saturates_without_overflow() {
        // A pathological remaining length must not overflow the multiply.
        assert_eq!(max_admissible_count(Some(usize::MAX)), Some(usize::MAX));
        assert!(!count_exceeds_remaining_input(usize::MAX, Some(usize::MAX)));
    }

    #[test]
    fn manual_test_leb128_write_read() {
        let mut buffer = Vec::new();
        leb128_write(300, &mut buffer);
        assert_eq!(buffer, vec![172, 2]);

        let mut reader = buffer.into_iter();
        let value = leb128_read(&mut reader).unwrap();
        assert_eq!(value, 300);
    }

    #[test]
    fn more_tests_leb128() {
        let testdata = vec![0, 1, 127, 128, 255, 256, 1234567890, 0xFFFFFFFFFFFFFFFF];
        let mut buffer = Vec::new();
        for &value in &testdata {
            leb128_write(value, &mut buffer);
        }
        let mut reader = buffer.into_iter();
        for &expected in &testdata {
            let value = leb128_read(&mut reader).unwrap();
            assert_eq!(value, expected);
        }
        assert!(
            reader.next().is_none(),
            "Reader should be empty after reading all values"
        );
    }
}
