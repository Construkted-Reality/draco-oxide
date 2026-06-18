use crate::core::shared::{CornerIdx, Cross, VertexIdx};
use crate::encode::entropy::rans::RabsCoder;
use crate::shared::attribute::octahedron_toolbox::OctahedronToolBox;
use crate::utils::bit_coder::leb128_write;

use super::PredictionSchemeImpl;
use crate::core::corner_table::GenericCornerTable;
use crate::core::{attribute::Attribute, shared::Vector};
use crate::prelude::{AttributeType, NdVector};

pub(crate) struct MeshNormalPrediction<'parents, C, const N: usize> {
    corner_table: &'parents C,
    pos: &'parents Attribute,
    flips: Vec<bool>,
}

impl<'parents, C, const N: usize> MeshNormalPrediction<'parents, C, N>
where
    C: GenericCornerTable,
    NdVector<N, i32>: Vector<N, Component = i32>,
{
    fn compute_normal_of_face(&self, c: CornerIdx, pos_c: NdVector<3, i32>) -> NdVector<3, i64> {
        // corners.
        let c_next = self.corner_table.next(c);
        let c_prev = self.corner_table.previous(c);

        let pos_next = self
            .pos
            .get::<NdVector<3, i32>, 3>(self.corner_table.point_idx(c_next));
        let pos_prev = self
            .pos
            .get::<NdVector<3, i32>, 3>(self.corner_table.point_idx(c_prev));

        // Widen positions to i64 BEFORE differencing/crossing. Google's
        // `MeshPredictionSchemeGeometricNormalPredictorArea::ComputePredictedValue`
        // (predictor_area.h:49-71) does all of delta/cross math on
        // `VectorD<int64_t, 3>` — computing the cross in i32 first overflows
        // for large quantized position deltas.
        let to_i64 = |v: NdVector<3, i32>| -> NdVector<3, i64> {
            let mut out = NdVector::<3, i64>::zero();
            *out.get_mut(0) = *v.get(0) as i64;
            *out.get_mut(1) = *v.get(1) as i64;
            *out.get_mut(2) = *v.get(2) as i64;
            out
        };
        let pos_c = to_i64(pos_c);
        let pos_next = to_i64(pos_next);
        let pos_prev = to_i64(pos_prev);

        // Compute the difference to next and prev (in i64).
        let delta_next = pos_next - pos_c;
        let delta_prev = pos_prev - pos_c;

        // Take the cross product (in i64).
        delta_next.cross(delta_prev)
    }
}

impl<'parents, C, const N: usize> PredictionSchemeImpl<'parents, C, N>
    for MeshNormalPrediction<'parents, C, N>
where
    C: GenericCornerTable,
    NdVector<N, i32>: Vector<N, Component = i32>,
{
    const ID: u32 = 2;

    type AdditionalDataForMetadata = ();

    fn new(parents: &[&'parents Attribute], corner_table: &'parents C) -> Self {
        assert!(parents.len() == 1, "MeshNormalPrediction requires exactly one parent attribute for position. but it has {} parents.", parents.len());
        assert!(
            parents[0].get_attribute_type() == AttributeType::Position,
            "MeshNormalPrediction requires the first parent attribute to be of type Position."
        );
        Self {
            corner_table,
            pos: parents[0], // we made sure that the first parent is the position attribute
            flips: Vec::new(),
        }
    }

    fn get_values_impossible_to_predict(
        &mut self,
        _seq: &mut Vec<std::ops::Range<usize>>,
    ) -> Vec<std::ops::Range<usize>> {
        unimplemented!();
    }

    fn predict(
        &mut self,
        c: CornerIdx,
        _vertices_up_till_now: &[VertexIdx],
        attribute: &Attribute,
    ) -> NdVector<N, i32> {
        let pos_c = self.pos.get(self.corner_table.point_idx(c));

        // Iterate the corners around the vertex in EXACTLY Google's
        // `VertexCornersIterator` order (corner_table_iterators.h:210-262):
        // start at `c`, swing-LEFT accumulating until a boundary (then switch
        // to swing-RIGHT from `c`) or until we loop back to `c`. Google's
        // GeometricNormalPredictorArea constructs the iterator from the corner
        // directly (`VertexCornersIterator cit(corner_table, corner_id)`), NOT
        // from the leftmost corner — and the boundary handling differs from a
        // "walk to leftmost, then swing right" approach for open vertices, so
        // we mirror the iterator verbatim. The decoder's `predict_normal` uses
        // the same order; the two MUST agree for byte-identical output.
        let mut sum = self.compute_normal_of_face(c, pos_c);
        let mut curr_c = c;
        loop {
            match self.corner_table.swing_left(curr_c) {
                Some(next) if next == c => break, // looped back: closed ring done.
                Some(next) => {
                    curr_c = next;
                    sum += self.compute_normal_of_face(curr_c, pos_c);
                }
                None => {
                    // Open boundary reached on the left — switch to swinging
                    // RIGHT from the start corner `c` until the other boundary.
                    let mut r = c;
                    while let Some(rn) = self.corner_table.swing_right(r) {
                        r = rn;
                        sum += self.compute_normal_of_face(r, pos_c);
                    }
                    break;
                }
            }
        }

        // Cap |sum| ≤ 2^29 so the i64→i32 cast is safe, exactly mirroring
        // Google's GeometricNormalPredictorArea::ComputePredictedValue
        // (predictor_area.h:83-101): in TRIANGLE_AREA mode `abs_sum`,
        // `quotient` and the division are all int64.
        let upper_bound: i64 = 1 << 29;
        let abs_sum = sum.get(0).abs() + sum.get(1).abs() + sum.get(2).abs();
        if abs_sum > upper_bound {
            let quotient = abs_sum / upper_bound;
            sum /= quotient;
        }
        let pred_normal_3d: [i32; 3] = [
            *sum.get(0) as i32,
            *sum.get(1) as i32,
            *sum.get(2) as i32,
        ];

        // The remainder mirrors Google's
        // `MeshPredictionSchemeGeometricNormalEncoder::ComputeCorrectionValues`
        // (mesh_prediction_scheme_geometric_normal_encoder.h:117-160). Normals
        // use an 8-bit octahedral grid (max_quantized_value = 255,
        // center_value = 127); the transform writes those two values.
        let tool = OctahedronToolBox::new(8);

        // Canonicalize the integer 3D normal so |x|+|y|+|z| == center_value.
        let mut p: [i64; 3] = [
            pred_normal_3d[0] as i64,
            pred_normal_3d[1] as i64,
            pred_normal_3d[2] as i64,
        ];
        tool.canonicalize_integer_vector(&mut p);

        // Octahedral coords for both possible directions (pos / neg).
        let pos = [p[0] as i32, p[1] as i32, p[2] as i32];
        let neg = [-pos[0], -pos[1], -pos[2]];
        let (pos_s, pos_t) = tool.integer_vector_to_quantized_octahedral_coords(pos);
        let (neg_s, neg_t) = tool.integer_vector_to_quantized_octahedral_coords(neg);

        // Original (actual) octahedral value of this corner's normal.
        let orig_v = attribute.get::<NdVector<2, i32>, 2>(self.corner_table.point_idx(c));
        let orig = [*orig_v.get(0), *orig_v.get(1)];

        // Correction for each candidate, then ModMax to bring into [-c, c].
        let pc_raw = tool.compute_correction(orig, [pos_s, pos_t]);
        let pc = [tool.mod_max(pc_raw[0]), tool.mod_max(pc_raw[1])];
        let nc_raw = tool.compute_correction(orig, [neg_s, neg_t]);
        let nc = [tool.mod_max(nc_raw[0]), tool.mod_max(nc_raw[1])];

        // Choose the direction with the smaller absolute-sum correction.
        // Ties go to the negative direction (matches Google's `<`).
        let pc_abs = pc[0].abs() + pc[1].abs();
        let nc_abs = nc[0].abs() + nc[1].abs();
        let chosen = if pc_abs < nc_abs {
            self.flips.push(false);
            pc
        } else {
            self.flips.push(true);
            nc
        };

        let mut out = NdVector::<N, i32>::zero();
        *out.get_mut(0) = tool.make_positive(chosen[0]);
        *out.get_mut(1) = tool.make_positive(chosen[1]);
        out
    }

    fn encode_prediction_metadtata<W>(&self, writer: &mut W) -> Result<(), super::Err>
    where
        W: crate::prelude::ByteWriter,
    {
        let freq_count_0 = self.flips.iter().filter(|&&o| !o).count();
        let zero_prob = (((freq_count_0 as f32 / self.flips.len() as f32) * 256.0 + 0.5) as u16)
            .clamp(1, 255) as u8;
        let mut rabs_coder: RabsCoder = RabsCoder::new(zero_prob as usize, None);
        writer.write_u8(zero_prob);
        // rABS is LIFO: Google's RAnsBitEncoder::EndEncoding encodes the bits in
        // reverse so the decoder pops them back in forward (vertex) order. Encode
        // flips reversed to (a) byte-match Google and (b) fix the round-trip — the
        // previous forward encode made the decoder read the flips reversed.
        for &b in self.flips.iter().rev() {
            // Encode each flip as a single bit
            rabs_coder.write(if b { 1 } else { 0 })?;
        }
        let buffer = rabs_coder.flush()?;
        leb128_write(buffer.len() as u64, writer);
        for byte in buffer {
            writer.write_u8(byte);
        }
        Ok(())
    }
}
