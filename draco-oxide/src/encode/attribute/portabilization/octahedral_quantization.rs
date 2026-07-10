use crate::core::shared::AttributeValueIdx;
use crate::core::shared::DataValue;
use crate::core::shared::Vector;
use crate::prelude::Attribute;
use crate::prelude::AttributeType;
use crate::prelude::ByteWriter;
use crate::prelude::NdVector;
use crate::shared::attribute::octahedron_toolbox::OctahedronToolBox;
use crate::shared::attribute::Portable;

use super::Config;
use super::PortabilizationImpl;

pub struct OctahedralQuantization<Data, const N: usize> {
    /// iterator over the attribute values.
    /// this is not 'Vec<_>' because we want to nicely consume the data.
    att: Attribute,

    /// the size of the quantization
    quantization_bits: u8,

    _marker: std::marker::PhantomData<Data>,
}

impl<Data, const N: usize> OctahedralQuantization<Data, N>
where
    Data: Vector<N>,
    NdVector<N, i32>: Vector<N, Component = i32>,
{
    pub fn new<W>(att: Attribute, cfg: Config, writer: &mut W) -> Self
    where
        W: ByteWriter,
    {
        assert!(
            att.get_attribute_type() == AttributeType::Normal,
            "Octahedral quantization can only be applied to normal attributes."
        );

        // encode the quantization bits.
        writer.write_u8(cfg.quantization_bits);

        Self {
            att,
            quantization_bits: cfg.quantization_bits,
            _marker: std::marker::PhantomData,
        }
    }

    fn portabilize_value(&mut self, val: Data) -> NdVector<2, i32> {
        // Round-half-up integer octahedral quantization, byte-exact with Google
        // (FloatVectorToQuantizedOctahedralCoords). The previous path projected
        // to float and truncated, producing off-by-one (s,t) vs Google.
        let toolbox = OctahedronToolBox::new(self.quantization_bits);
        let v = unsafe {
            [
                val.get_unchecked(0).to_f64(),
                val.get_unchecked(1).to_f64(),
                val.get_unchecked(2).to_f64(),
            ]
        };
        let (s, t) = toolbox.float_vector_to_quantized_octahedral_coords(v);
        NdVector::<2, i32>::from([s, t])
    }
}

impl<Data, const N: usize> PortabilizationImpl<N> for OctahedralQuantization<Data, N>
where
    Data: Vector<N> + Portable,
    NdVector<N, i32>: Vector<N, Component = i32>,
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
