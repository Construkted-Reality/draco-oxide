use crate::core::shared::{AttributeValueIdx, DataValue, Vector};
use crate::prelude::{Attribute, ByteWriter, NdVector};
use crate::shared::attribute::Portable;

use super::{Config, PortabilizationImpl};

pub(crate) struct QuantizationCoordinateWise<Data, const N: usize>
where
    Data: Vector<N>,
{
    att: Attribute,
    range_size: f32,
    min_values: NdVector<N, f32>,
    quantization_bits: u8,
    _phantom: std::marker::PhantomData<Data>,
}

impl<Data, const N: usize> QuantizationCoordinateWise<Data, N>
where
    NdVector<N, i32>: Vector<N, Component = i32>,
    NdVector<N, f32>: Vector<N, Component = f32> + Portable,
    Data: Vector<N> + Portable,
    Data::Component: DataValue,
{
    pub fn new<W>(att: Attribute, cfg: Config, writer: &mut W) -> Self
    where
        W: ByteWriter,
    {
        let (min_values, range_size, quantization_bits) = match cfg.explicit_quantization {
            Some(eq) => {
                // Caller-supplied lattice — skip the per-mesh scan and use the
                // values directly. The bitstream metadata slot is unchanged;
                // only the source of the values differs.
                assert_eq!(
                    eq.origin.len(),
                    N,
                    "ExplicitQuantization.origin.len() ({}) must equal attribute num_components ({})",
                    eq.origin.len(),
                    N,
                );
                let mut min_values = NdVector::<N, f32>::zero();
                for i in 0..N {
                    *min_values.get_mut(i) = eq.origin[i];
                }
                (min_values, eq.range, eq.quantization_bits)
            }
            None => {
                // Per-mesh scan. Seed min/max from the FIRST value, not from
                // zero. Seeding from `NdVector::zero()` (the original behavior)
                // forced the quantization cube to include the origin (0,0,0):
                // `min` could never rise above 0 and `max` could never fall
                // below 0. For geometry offset far from the origin — e.g. a
                // 3D-Tiles tile expressed in a local frame hundreds of metres
                // from zero — this inflated `delta_max` to the distance from
                // the origin to the far corner (often 1.5–3x the true extent),
                // wasting most quantization levels on empty space and crushing
                // precision on the thinnest axis. Seeding from the first vertex
                // makes the cube tight to the data. The decoder reads the
                // transmitted `min_values`/`range`, so round-trip stays correct.
                let vals = att.unique_vals_as_slice::<Data>();
                let mut min_values = NdVector::<N, f32>::zero();
                let mut max_values = NdVector::<N, f32>::zero();
                if let Some(first) = vals.first() {
                    for i in 0..N {
                        let component = first.get(i).to_f64() as f32;
                        *min_values.get_mut(i) = component;
                        *max_values.get_mut(i) = component;
                    }
                    for val in vals {
                        for i in 0..N {
                            let component = val.get(i).to_f64() as f32;
                            if component < *min_values.get(i) {
                                *min_values.get_mut(i) = component;
                            }
                            if component > *max_values.get(i) {
                                *max_values.get_mut(i) = component;
                            }
                        }
                    }
                }
                // Empty attribute: min/max stay zero, delta_max becomes 0, and
                // the zero-range path in `portabilize_value` handles it.

                let mut delta_max = 0.0;
                for i in 0..N {
                    let delta = *max_values.get(i) - *min_values.get(i);
                    if delta > delta_max {
                        delta_max = delta;
                    }
                }
                (min_values, delta_max, cfg.quantization_bits)
            }
        };

        // write metadata
        min_values.write_to(writer);
        range_size.write_to(writer);
        writer.write_u8(quantization_bits);

        Self {
            att,
            range_size,
            min_values,
            quantization_bits,
            _phantom: std::marker::PhantomData,
        }
    }

    fn portabilize_value(&mut self, val: Data) -> NdVector<N, i32> {
        // convert value to float vector TODO: implement the vector conversion so that this will be one line
        let val: NdVector<N, f32> = {
            let mut out = NdVector::<N, f32>::zero();
            for i in 0..N {
                *out.get_mut(i) = val.get(i).to_f64() as f32;
            }
            out
        };
        // Match Google's `Quantizer` byte-for-byte
        // (core/quantization_utils.{h,cc} + attribute_quantization_transform.cc
        // GeneratePortableAttribute): precompute a single f32 reciprocal
        //   inverse_delta = max_quantized_value / range
        // then quantize each component as
        //   q = floor((value - min) * inverse_delta + 0.5)
        // Doing the divide per-value and multiplying afterwards (the previous
        // `(diff / range) * max_q`) rounds differently in f32 and produced
        // off-by-one quantized positions at a handful of vertices, which then
        // cascaded into wrong predicted normals. A zero range maps everything
        // to 0 (Google bumps range to 1.0; both yield q=0 since diff=0).
        let max_quantized_value = f32::from_u64((1 << self.quantization_bits) - 1);
        let inverse_delta = if self.range_size == 0.0 {
            0.0
        } else {
            max_quantized_value / self.range_size
        };
        let diff = val - self.min_values;
        let mut out = NdVector::<N, i32>::zero();
        for i in 0..N {
            let v = *diff.get(i) * inverse_delta;
            *out.get_mut(i) = (v + 0.5).floor() as i32;
        }
        out
    }
}

impl<Data, const N: usize> PortabilizationImpl<N> for QuantizationCoordinateWise<Data, N>
where
    NdVector<N, i32>: Vector<N, Component = i32>,
    NdVector<N, f32>: Vector<N, Component = f32> + Portable,
    Data: Vector<N> + Portable,
{
    fn portabilize(mut self) -> Attribute {
        let mut out = Vec::new();
        for i in 0..self.att.num_unique_values() {
            let i = AttributeValueIdx::from(i);
            out.push(self.portabilize_value(self.att.get_unique_val::<Data, N>(i)));
        }
        let mut port_att = Attribute::from_without_removing_duplicates(
            self.att.get_id(),
            out,
            self.att.get_attribute_type(),
            self.att.get_domain(),
            self.att.get_parents().clone(),
        );
        port_att.set_point_to_att_val_map(self.att.take_point_to_att_val_map());
        port_att
    }
}
