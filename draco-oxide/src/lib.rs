// lib.rs

/// Contains the interface between `Mesh` object and 3D geometry files
/// such as obj and gltf.
pub mod io;

/// Contains compression techniques used by the encoder and the decoder.
pub(crate) mod shared;

/// Defines the mesh encoder.
pub mod encode;

/// Defines the decoders.
pub mod decode;

/// Contains the shared definitions, native objects, and the buffer.
pub(crate) mod core;

/// Contains the macros used by the encoder and the decoder.
pub(crate) mod utils;

/// Contains the most commonly used traits, types, and objects.
pub mod prelude {
    pub use crate::core::attribute::{Attribute, AttributeDomain, AttributeType};
    pub use crate::core::bit_coder::{
        ByteReader, ByteWriter, FunctionalByteReader, FunctionalByteWriter,
    };
    pub use crate::core::mesh::{builder::MeshBuilder, Mesh};
    pub use crate::core::shared::ConfigType;
    pub use crate::core::shared::{DataValue, NdVector, PointIdx, Vector};
    pub use crate::encode::{self, encode};
    pub use crate::core::attribute::ComponentDataType;
    pub use crate::decode::{
        self, decode, decode_to_raw, decode_to_raw_with_warnings, decode_with_warnings,
        DecodeWarning, DecodedRaw, RawAttribute,
    };
    pub use crate::io::gltf::draco_decoder::splice_glb_remove_draco;
}

/// Evaluation module contains the evaluation functions for the encoder and the decoder.
/// When enabled, draco-oxide encoder will spit out the evaluation data mixed with encoded data,
/// and then the `EvalWriter` is used to filter out the evaluation data. This functionality is
/// most often used in the development and testing phase.
#[cfg(feature = "evaluation")]
pub mod eval;
