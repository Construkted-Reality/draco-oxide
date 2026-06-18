pub mod config;
pub(crate) mod edgebreaker;
pub(crate) mod sequential;

use std::fmt::Debug;

use crate::core::bit_coder::ByteWriter;
use crate::core::shared::{ConfigType, PointIdx};
use crate::encode::connectivity::edgebreaker::{DefaultTraversal, ValenceTraversal};
use crate::prelude::{Attribute, AttributeType};
use crate::shared::connectivity::edgebreaker::EdgebreakerKind;

#[cfg(feature = "evaluation")]
use crate::eval;

/// entry point for encoding connectivity.
pub fn encode_connectivity<'faces, W>(
    faces: &'faces [[PointIdx; 3]],
    atts: &mut [Attribute],
    writer: &mut W,
    cfg: &super::Config,
) -> Result<ConnectivityEncoderOutput<'faces>, Err>
where
    W: ByteWriter,
{
    #[cfg(feature = "evaluation")]
    eval::scope_begin("connectivity info", writer);

    // Select the Edgebreaker traversal the way Google's InitializeEncoder does
    // (mesh_edgebreaker_encoder.cc:34-45): VALENCE when num_faces >= 1000 and
    // speed < 5, else STANDARD. speed = 10 - compression_level.
    let speed = 10u8.saturating_sub(cfg.compression_level);
    let traversal = if faces.len() < 1000 || speed >= 5 {
        EdgebreakerKind::Standard
    } else {
        EdgebreakerKind::Valence
    };
    let conn_cfg = Config::Edgebreaker(edgebreaker::Config {
        traversal,
        use_single_connectivity: false,
    });

    let result = encode_connectivity_datatype_unpacked(faces, atts, writer, conn_cfg);

    #[cfg(feature = "evaluation")]
    eval::scope_end(writer);
    result
}

pub fn encode_connectivity_datatype_unpacked<'faces, W>(
    faces: &'faces [[PointIdx; 3]],
    atts: &mut [Attribute],
    writer: &mut W,
    cfg: Config,
) -> Result<ConnectivityEncoderOutput<'faces>, Err>
where
    W: ByteWriter,
{
    let result = match cfg {
        Config::Edgebreaker(cfg) => {
            #[cfg(feature = "evaluation")]
            eval::scope_begin("edgebreaker", writer);

            let result = match cfg.traversal {
                EdgebreakerKind::Standard => {
                    let encoder =
                        edgebreaker::Edgebreaker::<DefaultTraversal>::new(cfg, atts, faces)?;
                    encoder.encode_connectivity(faces, writer)
                }
                EdgebreakerKind::Predictive => {
                    unimplemented!("Predictive edgebreaker encoding is not implemented yet");
                }
                EdgebreakerKind::Valence => {
                    let encoder =
                        edgebreaker::Edgebreaker::<ValenceTraversal>::new(cfg, atts, faces)?;
                    encoder.encode_connectivity(faces, writer)
                }
            };

            #[cfg(feature = "evaluation")]
            eval::scope_end(writer);

            result.map(ConnectivityEncoderOutput::Edgebreaker)?
        }
        Config::Sequential(cfg) => {
            #[cfg(feature = "evaluation")]
            eval::scope_begin("sequential", writer);

            let num_points = atts
                .iter()
                .find(|att| att.get_attribute_type() == AttributeType::Position)
                .unwrap()
                .len();
            let encoder = sequential::Sequential::new(cfg, num_points);
            encoder.encode_connectivity(faces, writer)?;

            #[cfg(feature = "evaluation")]
            eval::scope_end(writer);

            ConnectivityEncoderOutput::Sequential(())
        }
    };
    Ok(result)
}

pub trait ConnectivityEncoder {
    type Err;
    type Config;
    type Output;
    fn encode_connectivity<W>(
        self,
        faces: &[[PointIdx; 3]],
        buffer: &mut W,
    ) -> Result<Self::Output, Self::Err>
    where
        W: ByteWriter;
}

pub(crate) enum ConnectivityEncoderOutput<'faces> {
    Edgebreaker(edgebreaker::Output<'faces>),
    Sequential(()),
}

#[remain::sorted]
#[derive(thiserror::Error, Debug)]
pub enum Err {
    #[error("Edgebreaker encoding error: {0}")]
    EdgebreakerError(#[from] edgebreaker::Err),
    #[error("Position attribute must be of type f32 or f64")]
    PositionAttributeTypeError,
    #[error("Sequential encoding error: {0}")]
    SequentialError(#[from] sequential::Err),
    #[error("Too many connectivity attributes")]
    TooManyConnectivityAttributes,
}

#[remain::sorted]
#[derive(Clone, Debug)]
pub enum Config {
    Edgebreaker(edgebreaker::Config),
    #[allow(unused)] // we currently support only edgebreaker
    Sequential(sequential::Config),
}

impl ConfigType for Config {
    fn default() -> Self {
        Self::Edgebreaker(edgebreaker::Config::default())
    }
}
