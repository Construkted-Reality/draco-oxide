//! Integer octahedral toolbox, ported from Google Draco's `OctahedronToolBox`
//! (`compression/attributes/normal_compression_utils.h`). Byte-exact with the
//! C++ encoder: round-half-up quantization + integer abs-sum canonicalization,
//! replacing draco-oxide's earlier float-projection-and-truncate path.
//!
//! The quantization half (`float_vector_to_quantized_octahedral_coords` and its
//! helpers) is wired into the normal portabilization. The correction/flip half
//! (`compute_correction`, `mod_max`, `make_positive`, the diamond/rotation
//! helpers, `canonicalize_integer_vector`) is the foundation for replacing the
//! float-based normal prediction+transform path (see
//! docs/audit/2026-06-17-google-parity/10-normal-byte-identity-plan.md) and is
//! not yet called — hence the module-wide dead-code allowance.
#![allow(dead_code)]

/// `q`-bit octahedral parameter space:
/// * `max_quantized_value = 2^q - 1` (odd)
/// * `max_value = max_quantized_value - 1` (even, the diamond edge)
/// * `center_value = max_value / 2`
#[derive(Debug, Clone, Copy)]
pub(crate) struct OctahedronToolBox {
    max_quantized_value: i32,
    max_value: i32,
    center_value: i32,
}

impl OctahedronToolBox {
    pub(crate) fn new(quantization_bits: u8) -> Self {
        let max_quantized_value = (1i32 << quantization_bits) - 1;
        let max_value = max_quantized_value - 1;
        let center_value = max_value / 2;
        Self {
            max_quantized_value,
            max_value,
            center_value,
        }
    }

    pub(crate) fn center_value(&self) -> i32 {
        self.center_value
    }
    pub(crate) fn max_value(&self) -> i32 {
        self.max_value
    }
    #[allow(dead_code)]
    pub(crate) fn max_quantized_value(&self) -> i32 {
        self.max_quantized_value
    }

    /// Mirror of `CanonicalizeOctahedralCoords` — fold the top-left/bottom-right
    /// quadrant edges into bottom-left/top-right, and corners to the top-right.
    pub(crate) fn canonicalize_octahedral_coords(&self, s: i32, t: i32) -> (i32, i32) {
        let (max, center) = (self.max_value, self.center_value);
        let (mut s, mut t) = (s, t);
        if (s == 0 && t == 0) || (s == 0 && t == max) || (s == max && t == 0) {
            s = max;
            t = max;
        } else if s == 0 && t > center {
            t = center - (t - center);
        } else if s == max && t < center {
            t = center + (center - t);
        } else if t == max && s < center {
            s = center + (center - s);
        } else if t == 0 && s > center {
            s = center - (s - center);
        }
        (s, t)
    }

    /// Mirror of `IntegerVectorToQuantizedOctahedralCoords`.
    /// Precondition: `|v0| + |v1| + |v2| == center_value`.
    pub(crate) fn integer_vector_to_quantized_octahedral_coords(&self, v: [i32; 3]) -> (i32, i32) {
        let (max, center) = (self.max_value, self.center_value);
        let (s, t);
        if v[0] >= 0 {
            // Right hemisphere.
            s = v[1] + center;
            t = v[2] + center;
        } else {
            // Left hemisphere.
            s = if v[1] < 0 { v[2].abs() } else { max - v[2].abs() };
            t = if v[2] < 0 { v[1].abs() } else { max - v[1].abs() };
        }
        self.canonicalize_octahedral_coords(s, t)
    }

    /// Mirror of `FloatVectorToQuantizedOctahedralCoords` — round-half-up.
    pub(crate) fn float_vector_to_quantized_octahedral_coords(&self, vector: [f64; 3]) -> (i32, i32) {
        let abs_sum = vector[0].abs() + vector[1].abs() + vector[2].abs();
        let scaled = if abs_sum > 1e-6 {
            let scale = 1.0 / abs_sum;
            [vector[0] * scale, vector[1] * scale, vector[2] * scale]
        } else {
            [1.0, 0.0, 0.0]
        };
        let cv = self.center_value as f64;
        let mut iv = [
            (scaled[0] * cv + 0.5).floor() as i32,
            (scaled[1] * cv + 0.5).floor() as i32,
            0i32,
        ];
        // Make sure the abs sum is exactly center_value.
        iv[2] = self.center_value - iv[0].abs() - iv[1].abs();
        if iv[2] < 0 {
            if iv[1] > 0 {
                iv[1] += iv[2];
            } else {
                iv[1] -= iv[2];
            }
            iv[2] = 0;
        }
        if scaled[2] < 0.0 {
            iv[2] = -iv[2];
        }
        self.integer_vector_to_quantized_octahedral_coords(iv)
    }

    /// `s`,`t` expected with center already at origin. Mirror of `IsInDiamond`.
    pub(crate) fn is_in_diamond(&self, s: i32, t: i32) -> bool {
        (s.unsigned_abs() + t.unsigned_abs()) as i32 <= self.center_value
    }

    /// Mirror of `InvertDiamond` (unsigned-wrapping arithmetic, then /2).
    pub(crate) fn invert_diamond(&self, p: &mut [i32; 2]) {
        let (s, t) = (p[0], p[1]);
        let (sign_s, sign_t) = if s >= 0 && t >= 0 {
            (1i32, 1i32)
        } else if s <= 0 && t <= 0 {
            (-1, -1)
        } else {
            (if s > 0 { 1 } else { -1 }, if t > 0 { 1 } else { -1 })
        };
        let corner_point_s = (sign_s * self.center_value) as u32;
        let corner_point_t = (sign_t * self.center_value) as u32;
        let mut us = s as u32;
        let mut ut = t as u32;
        us = us.wrapping_add(us).wrapping_sub(corner_point_s);
        ut = ut.wrapping_add(ut).wrapping_sub(corner_point_t);
        if sign_s * sign_t >= 0 {
            let temp = us;
            us = 0u32.wrapping_sub(ut);
            ut = 0u32.wrapping_sub(temp);
        } else {
            std::mem::swap(&mut us, &mut ut);
        }
        us = us.wrapping_add(corner_point_s);
        ut = ut.wrapping_add(corner_point_t);
        p[0] = (us as i32) / 2;
        p[1] = (ut as i32) / 2;
    }

    /// Mirror of `GetRotationCount`.
    fn get_rotation_count(&self, pred: [i32; 2]) -> i32 {
        let (sign_x, sign_y) = (pred[0], pred[1]);
        if sign_x == 0 {
            if sign_y == 0 {
                0
            } else if sign_y > 0 {
                3
            } else {
                1
            }
        } else if sign_x > 0 {
            if sign_y >= 0 {
                2
            } else {
                1
            }
        } else if sign_y <= 0 {
            0
        } else {
            3
        }
    }

    /// Mirror of `RotatePoint`.
    fn rotate_point(&self, p: [i32; 2], rotation_count: i32) -> [i32; 2] {
        match rotation_count {
            1 => [p[1], -p[0]],
            2 => [-p[0], -p[1]],
            3 => [-p[1], p[0]],
            _ => p,
        }
    }

    /// Mirror of `IsInBottomLeft`.
    fn is_in_bottom_left(&self, p: [i32; 2]) -> bool {
        if p[0] == 0 && p[1] == 0 {
            return true;
        }
        p[0] < 0 && p[1] <= 0
    }

    /// Mirror of `ModMax` (correction-value canonicalization).
    pub(crate) fn mod_max(&self, x: i32) -> i32 {
        if x > self.center_value {
            x - self.max_quantized_value
        } else if x < -self.center_value {
            x + self.max_quantized_value
        } else {
            x
        }
    }

    /// Mirror of `MakePositive`.
    pub(crate) fn make_positive(&self, x: i32) -> i32 {
        if x < 0 {
            x + self.max_quantized_value
        } else {
            x
        }
    }

    /// Mirror of the canonicalized encoding transform's `ComputeCorrection`
    /// (`prediction_scheme_normal_octahedron_canonicalized_encoding_transform.h`).
    /// `orig`/`pred` are octahedral (s,t) in `[0, 2*center]`.
    pub(crate) fn compute_correction(&self, orig: [i32; 2], pred: [i32; 2]) -> [i32; 2] {
        let c = self.center_value;
        let mut orig = [orig[0] - c, orig[1] - c];
        let mut pred = [pred[0] - c, pred[1] - c];
        if !self.is_in_diamond(pred[0], pred[1]) {
            self.invert_diamond(&mut orig);
            self.invert_diamond(&mut pred);
        }
        if !self.is_in_bottom_left(pred) {
            let rc = self.get_rotation_count(pred);
            orig = self.rotate_point(orig, rc);
            pred = self.rotate_point(pred, rc);
        }
        [
            self.make_positive(orig[0] - pred[0]),
            self.make_positive(orig[1] - pred[1]),
        ]
    }

    /// Mirror of `CanonicalizeIntegerVector` — scale a signed integer vector so
    /// its abs sum equals `center_value`.
    pub(crate) fn canonicalize_integer_vector(&self, vec: &mut [i64; 3]) {
        let abs_sum = vec[0].abs() + vec[1].abs() + vec[2].abs();
        let center = self.center_value as i64;
        if abs_sum == 0 {
            vec[0] = center; // vec[1] == vec[2] == 0
            vec[1] = 0;
            vec[2] = 0;
        } else {
            vec[0] = (vec[0] * center) / abs_sum;
            vec[1] = (vec[1] * center) / abs_sum;
            if vec[2] >= 0 {
                vec[2] = center - vec[0].abs() - vec[1].abs();
            } else {
                vec[2] = -(center - vec[0].abs() - vec[1].abs());
            }
        }
    }
}
