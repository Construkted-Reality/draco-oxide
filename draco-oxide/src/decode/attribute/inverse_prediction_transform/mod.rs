//! Inverse of `encode/attribute/prediction_transform/`.
//!
//! Given a stream of decoded "positive i32" symbols + a per-attribute
//! prediction value, produces the original quantized i32 attribute value.
//! The inverse transform is the second-to-last decode step:
//!
//!   symbols → from_positive_i32 → pred + corr (with wrap) → quantized i32
//!                                                              ↓
//!                                                       deportabilize

use crate::core::bit_coder::ReaderErr;

#[derive(Debug, thiserror::Error)]
pub enum Err {
    #[error("Reader error: {0}")]
    Reader(#[from] ReaderErr),
    #[error("Invalid prediction transform id: {0}")]
    InvalidId(u8),
    #[error("OctahedralReflection / Orthogonal inverse transforms not yet implemented")]
    OctahedralTodo,
}

/// Mirrors `encode/attribute/prediction_transform/mod.rs::PredictionTransformType::get_id`:
///   0xFF → NoTransform
///   0    → Difference
///   1    → WrappedDifference
///   2    → OctahedralReflection
///   3    → OctahedralOrthogonal
///   4    → Orthogonal
#[derive(Debug, Clone, Copy)]
pub(crate) enum InverseTransformKind {
    NoTransform,
    Difference,
    WrappedDifference,
    OctahedralReflection,
    OctahedralOrthogonal,
    Orthogonal,
}

impl InverseTransformKind {
    pub(crate) fn from_id(id: u8) -> Result<Self, Err> {
        match id {
            0xFF => Ok(Self::NoTransform),
            0 => Ok(Self::Difference),
            1 => Ok(Self::WrappedDifference),
            2 => Ok(Self::OctahedralReflection),
            3 => Ok(Self::OctahedralOrthogonal),
            4 => Ok(Self::Orthogonal),
            _ => Err(Err::InvalidId(id)),
        }
    }
}

/// N-component inverse prediction transform. Used by:
/// - Position (N=3): WrappedDifference + 11-bit quantization.
/// - TextureCoordinate (N=2): WrappedDifference + 10-bit quantization.
/// - Custom (N=*): WrappedDifference + ToBits.
/// - Normal (N=2): handled by `OctahedralOrthogonalInverseTransform` below.
pub(crate) enum InverseTransform {
    NoTransform,
    Difference,
    WrappedDifference {
        min: i32,
        max: i32,
        max_diff: i32,
    },
}

impl InverseTransform {
    pub(crate) fn read<R: crate::prelude::ByteReader>(
        reader: &mut R,
        kind: InverseTransformKind,
    ) -> Result<Self, Err> {
        match kind {
            InverseTransformKind::NoTransform => Ok(Self::NoTransform),
            InverseTransformKind::Difference => Ok(Self::Difference),
            InverseTransformKind::WrappedDifference => {
                let min = read_i32(reader)?;
                let max = read_i32(reader)?;
                let max_diff = 1 + (max - min);
                Ok(Self::WrappedDifference { min, max, max_diff })
            }
            InverseTransformKind::OctahedralOrthogonal
            | InverseTransformKind::OctahedralReflection
            | InverseTransformKind::Orthogonal => Err(Err::OctahedralTodo),
        }
    }

    /// Applies the inverse transform per-component:
    /// `(corr_positive[N], pred[N]) → orig[N]`. `corr_positive` is the
    /// symbol value as decoded from the stream (still in zigzag form).
    pub(crate) fn inverse_n(
        &self,
        corr_positive: &[i32],
        pred: &[i32],
        out: &mut [i32],
    ) {
        debug_assert_eq!(corr_positive.len(), pred.len());
        debug_assert_eq!(corr_positive.len(), out.len());

        match self {
            Self::NoTransform => {
                for i in 0..corr_positive.len() {
                    out[i] = from_positive_i32(corr_positive[i]);
                }
            }
            Self::Difference => {
                for i in 0..corr_positive.len() {
                    out[i] = from_positive_i32(corr_positive[i]) + pred[i];
                }
            }
            Self::WrappedDifference {
                min,
                max,
                max_diff,
            } => {
                for i in 0..corr_positive.len() {
                    let corr = from_positive_i32(corr_positive[i]);
                    let pred_clamped = pred[i].clamp(*min, *max);
                    let mut val = pred_clamped + corr;
                    if val > *max {
                        val -= *max_diff;
                    } else if val < *min {
                        val += *max_diff;
                    }
                    out[i] = val;
                }
            }
        }
    }
}

/// `OctahedralOrthogonal` inverse for normals (always 2-component).
/// Mirrors `encode/attribute/prediction_transform/oct_orthogonal.rs`. Our
/// encoder hardcodes max=255 + center=127 (8-bit oct grid); Google may
/// emit a different `max_quantized_value` per the per-attribute
/// quantization bits, so we read both u32s and use them.
pub(crate) struct OctahedralOrthogonalInverseTransform {
    pub max_quantized_value: i32,
    pub center_value: i32,
}

impl OctahedralOrthogonalInverseTransform {
    pub(crate) fn read<R: crate::prelude::ByteReader>(reader: &mut R) -> Result<Self, Err> {
        let max_quantized_value = read_u32(reader)? as i32;
        let center_value = read_u32(reader)? as i32;
        Ok(Self {
            max_quantized_value,
            center_value,
        })
    }

    /// Inverse of the encoder's transform (see
    /// `encode/attribute/prediction_transform/oct_orthogonal.rs::map_with_tentative_metadata`).
    /// Both `pred` and `corr_positive` are 2-component i32 arrays in the
    /// `[0, max_quantized_value]` range.
    pub(crate) fn inverse(
        &self,
        corr_positive: &[i32; 2],
        pred: &[i32; 2],
    ) -> [i32; 2] {
        let center = self.center_value;
        let max = self.max_quantized_value;

        // Shift pred to centered coordinates [-center, center].
        let mut p0 = pred[0] - center;
        let mut p1 = pred[1] - center;

        // Encoder's "flip inside-out" transform on pred.
        let mut flipped = false;
        if p0.abs() + p1.abs() > center {
            flipped = true;
            let p0_old = p0;
            let quadrant_sign = -(p0 * p1).signum();
            p0 = quadrant_sign * p1 + p0.signum() * center;
            p1 = quadrant_sign * p0_old + p1.signum() * center;
        }

        // Rotate pred to lower-left quadrant; remember rotation count.
        let mut rotation_count = 0i32;
        if !(p0 == 0 && p1 == 0) {
            while p0 >= 0 || p1 > 0 {
                let tmp = p0;
                p0 = -p1;
                p1 = tmp;
                rotation_count += 1;
            }
        }

        // OctahedralOrthogonal encoder writes `corr = (orig - pred) mod max`
        // directly (negatives become `+ max`), NOT via zigzag/`to_positive_i32`.
        // So pass raw symbol values; `mod_max` wraps the sum back into
        // [-center, center]. Mirrors Google's `OctahedronToolBox::ModMax`.
        let mut o0 = mod_max(p0 + corr_positive[0], center, max);
        let mut o1 = mod_max(p1 + corr_positive[1], center, max);

        // Reverse rotation: rotate counter-clockwise rotation_count times.
        for _ in 0..rotation_count {
            let tmp = o0;
            o0 = o1;
            o1 = -tmp;
        }

        if flipped {
            // Reverse the flip-inside-out.
            let o0_old = o0;
            let quadrant_sign = -(o0 * o1).signum();
            o0 = quadrant_sign * o1 + o0.signum() * center;
            o1 = quadrant_sign * o0_old + o1.signum() * center;
        }

        [o0 + center, o1 + center]
    }
}

/// Mirrors Google's `OctahedronToolBox::ModMax`:
///   if x > center: return x - max
///   if x < -center: return x + max
///   else: return x
#[inline]
fn mod_max(x: i32, center: i32, max: i32) -> i32 {
    if x > center {
        x - max
    } else if x < -center {
        x + max
    } else {
        x
    }
}

/// Inverse of `utils::to_positive_i32`:
///   0 → 0,  1 → -1,  2 → 1,  3 → -2,  4 → 2, …
#[inline]
pub(crate) fn from_positive_i32(p: i32) -> i32 {
    if p & 1 == 0 {
        p >> 1
    } else {
        -((p >> 1) + 1)
    }
}

fn read_i32<R: crate::prelude::ByteReader>(reader: &mut R) -> Result<i32, ReaderErr> {
    let bytes = [
        reader.read_u8()?,
        reader.read_u8()?,
        reader.read_u8()?,
        reader.read_u8()?,
    ];
    Ok(i32::from_le_bytes(bytes))
}

fn read_u32<R: crate::prelude::ByteReader>(reader: &mut R) -> Result<u32, ReaderErr> {
    let bytes = [
        reader.read_u8()?,
        reader.read_u8()?,
        reader.read_u8()?,
        reader.read_u8()?,
    ];
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local copy of `utils::to_positive_i32` (which is `pub(crate)` in
    /// another module) — duplicating to avoid coupling test to private API.
    fn to_positive_i32(val: i32) -> i32 {
        if val >= 0 {
            val << 1
        } else {
            (-(val + 1) << 1) + 1
        }
    }

    #[test]
    fn from_positive_i32_round_trips() {
        for v in -50..=50i32 {
            let p = to_positive_i32(v);
            assert_eq!(from_positive_i32(p), v, "round-trip failed for {}", v);
        }
    }

    #[test]
    fn from_positive_i32_known_values() {
        assert_eq!(from_positive_i32(0), 0);
        assert_eq!(from_positive_i32(1), -1);
        assert_eq!(from_positive_i32(2), 1);
        assert_eq!(from_positive_i32(3), -2);
        assert_eq!(from_positive_i32(4), 2);
        assert_eq!(from_positive_i32(99), -50);
        assert_eq!(from_positive_i32(100), 50);
    }
}
