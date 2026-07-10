use super::PredictionTransformImpl;
use crate::core::shared::{NdVector, Vector};
use crate::prelude::ByteWriter;

pub struct OctahedronOrthogonalTransform<const N: usize> {
    out: Vec<NdVector<N, i32>>,
}

impl<const N: usize> OctahedronOrthogonalTransform<N> {
    pub fn new(_cfg: super::Config) -> Self {
        Self { out: Vec::new() }
    }
}

impl<const N: usize> PredictionTransformImpl<N> for OctahedronOrthogonalTransform<N> {
    fn map_with_tentative_metadata(&mut self, _orig: NdVector<N, i32>, pred: NdVector<N, i32>)
    where
        NdVector<N, i32>: Vector<N, Component = i32>,
    {
        // Safety:
        // We made sure that the data is two dimensional.
        assert!(N == 2,);

        // `MeshNormalPrediction::predict` now performs the full canonicalized
        // octahedral ComputeCorrection (Google's
        // mesh_prediction_scheme_geometric_normal_encoder.h) and returns the
        // final, MakePositive'd correction directly. This transform therefore
        // must NOT re-transform — it just records the correction so it becomes
        // the symbol. (`_orig` is unused; `pred` already holds the correction.)
        self.out.push(pred);
    }

    fn squeeze<W>(self, writer: &mut W) -> Vec<NdVector<N, i32>>
    where
        W: ByteWriter,
    {
        // write the max quantized value.
        writer.write_u32(255);
        // write center of the octahedron.
        writer.write_u32(255 / 2);

        self.out
    }
}
