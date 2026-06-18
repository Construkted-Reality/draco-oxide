pub(crate) mod attribute_encoder;
pub(crate) mod portabilization;
pub(crate) mod prediction_transform;

use crate::encode::attribute::portabilization::PortabilizationType;
use crate::encode::connectivity::ConnectivityEncoderOutput;
#[cfg(feature = "evaluation")]
use crate::eval;

use crate::prelude::{Attribute, ByteWriter, ConfigType};
use crate::shared::connectivity::edgebreaker::TraversalType;

pub fn encode_attributes<W>(
    atts: Vec<Attribute>,
    writer: &mut W,
    conn_out: ConnectivityEncoderOutput<'_>,
    cfg: &super::Config,
) -> Result<(), Err>
where
    W: ByteWriter,
{
    #[cfg(feature = "evaluation")]
    eval::scope_begin("attributes", writer);

    // Write the number of attribute encoders/decoders (In draco-oxide, this is the same as the number of attributes as
    // each attribute has its own encoder/decoder)
    writer.write_u8(atts.len() as u8);
    #[cfg(feature = "evaluation")]
    eval::write_json_pair("attributes count", atts.len().into(), writer);

    // The all-inclusive corner table carries per-attribute seam info, which
    // determines the on-wire element type below.
    let all_ct = match &conn_out {
        ConnectivityEncoderOutput::Edgebreaker(out) => Some(&out.corner_table),
        ConnectivityEncoderOutput::Sequential(_) => None,
    };
    for (i, att) in atts.iter().enumerate() {
        if cfg.encoder_method == crate::shared::header::EncoderMethod::Edgebreaker {
            // encode decoder id
            writer.write_u8((i as u8).wrapping_sub(1));
            // Encode the *element type* of the attribute encoder. This mirrors
            // Google's MeshEdgebreakerEncoderImpl::EncodeAttributesEncoderIdentifier:
            // a corner attribute whose corner table has no interior (non-boundary)
            // seams is encoded as a per-vertex encoder (MESH_VERTEX_ATTRIBUTE = 0),
            // not a per-corner encoder (MESH_CORNER_ATTRIBUTE = 1). Position-domain
            // attributes are always per-vertex. The decoder uses the universal /
            // attribute corner table for the actual traversal regardless, so this
            // field is metadata; we keep it byte-identical to Google.
            let element_type = match att.get_domain() {
                crate::core::attribute::AttributeDomain::Position => {
                    crate::core::attribute::AttributeDomain::Position
                }
                crate::core::attribute::AttributeDomain::Corner => {
                    // no_interior_seams == Some(true) downgrades to per-vertex.
                    match all_ct.and_then(|ct| ct.no_interior_seams(i)) {
                        Some(true) => crate::core::attribute::AttributeDomain::Position,
                        _ => crate::core::attribute::AttributeDomain::Corner,
                    }
                }
            };
            element_type.write_to(writer);
            // write traversal method for attribute encoding/decoding sequencer. We currently only support depth-first traversal.
            TraversalType::DepthFirst.write_to(writer);
        }
    }

    #[cfg(feature = "evaluation")]
    eval::array_scope_begin("attributes", writer);

    let mut port_atts: Vec<Attribute> = Vec::new();
    for att in &atts {
        // Write 1 to indicate that the encoder is for one attribute.
        writer.write_u8(1);

        att.get_attribute_type().write_to(writer);
        att.get_component_type().write_to(writer);
        writer.write_u8(att.get_num_components() as u8);
        writer.write_u8(0); // Normalized flag, currently not used.
        writer.write_u8(att.get_id().as_usize() as u8); // unique id

        // write the decoder type.
        PortabilizationType::default_for(att.get_attribute_type()).write_to(writer);
    }

    for (i, att) in atts.into_iter().enumerate() {
        #[cfg(feature = "evaluation")]
        eval::scope_begin("attribute", writer);

        let parents_ids = att.get_parents();
        let parents = parents_ids
            .iter()
            .map(|id| port_atts.iter().find(|att| att.get_id() == *id).unwrap())
            .collect::<Vec<_>>();

        let ty = att.get_attribute_type();
        let len = att.len();
        let mut enc_cfg = attribute_encoder::Config::default_for(ty, len);
        // If the caller supplied an explicit-quantization entry for this
        // attribute type via encode::Config::set_attribute_explicit_quantization,
        // thread it down into the nested portabilization::Config.
        if let Some(eq) = cfg.explicit_quantization.get(&ty) {
            if let Some(group) = enc_cfg.group_cfgs.first_mut() {
                group
                    .prediction_transform
                    .portabilization
                    .explicit_quantization = Some(eq.clone());
            }
        } else if let Some(&bits) = cfg.quantization_bits.get(&ty) {
            // Override only the bit count; the automatic per-mesh bbox scan
            // still runs. Skipped above when an explicit lattice is present
            // (that lattice carries its own bit count). Recorded as a sentinel
            // override so it survives the `default_for` rebuild of por_cfg in
            // attribute_encoder (which would otherwise reset the per-type
            // default bits).
            if let Some(group) = enc_cfg.group_cfgs.first_mut() {
                group
                    .prediction_transform
                    .portabilization
                    .quantization_bits_override = Some(bits);
            }
        }
        let encoder =
            attribute_encoder::AttributeEncoder::new(att, i, &parents, &conn_out, writer, enc_cfg);

        let port_att = encoder.encode::<true, false>()?;
        port_atts.push(port_att);

        #[cfg(feature = "evaluation")]
        eval::scope_end(writer);
    }

    #[cfg(feature = "evaluation")]
    {
        eval::array_scope_end(writer);
        eval::scope_end(writer);
    }

    Ok(())
}

#[derive(Clone, Debug)]
pub struct Config {
    #[allow(unused)]
    // This field is unused in the current implementation, as we only support the default attribute encoder configuration.
    cfgs: Vec<attribute_encoder::Config>,
}

impl ConfigType for Config {
    fn default() -> Self {
        Self {
            cfgs: vec![attribute_encoder::Config::default()],
        }
    }
}

#[remain::sorted]
#[derive(thiserror::Error, Debug)]
pub enum Err {
    #[error("Attribute encoding error: {0}")]
    AttributeError(#[from] attribute_encoder::Err),
}
