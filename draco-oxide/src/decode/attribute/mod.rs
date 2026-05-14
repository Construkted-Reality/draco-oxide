//! Attribute decoder.
//!
//! Mirrors `encode/attribute/mod.rs::encode_attributes`. The byte layout this
//! file decodes from is, in order:
//!
//! 1. `u8`  — number of attributes (N).
//! 2. For each attribute (Edgebreaker only):
//!    - `u8` — decoder id (encoder writes `(i as u8).wrapping_sub(1)`).
//!    - `u8` — `AttributeDomain` (Position=0, Corner=1).
//!    - `u8` — `TraversalType` (DepthFirst=0).
//! 3. For each attribute:
//!    - `u8` — attr count in this decoder (always 1).
//!    - `u8` — `AttributeType`.
//!    - `u8` — `ComponentDataType`.
//!    - `u8` — num components.
//!    - `u8` — normalized flag (currently always 0).
//!    - `u8` — unique id.
//!    - `u8` — `PortabilizationType`.
//! 4. For each attribute (the per-attribute encoded payload):
//!    - `u8` — prediction scheme type id.
//!    - `u8` — prediction transform type id.
//!    - …component-type-specific encoded data + metadata.

pub(crate) mod inverse_prediction_transform;
pub(crate) mod portabilization;

use crate::core::attribute::{
    Attribute, AttributeDomain, AttributeId, AttributeType, ComponentDataType,
};
use crate::core::bit_coder::ReaderErr;
use crate::core::corner_table::GenericCornerTable;
use crate::core::shared::{CornerIdx, NdVector, Vector};
use crate::decode::connectivity::DecoderCornerTable;
use crate::decode::entropy::symbol_coding;
use crate::decode::header::Header;
use crate::prelude::ByteReader;
use crate::shared::attribute::sequence::Traverser;
use crate::shared::header::EncoderMethod;

use self::inverse_prediction_transform::{
    InverseTransform, InverseTransformKind, OctahedralOrthogonalInverseTransform,
};
use self::portabilization::{
    DeportabilizationKind, OctahedralNormal, Quantization,
};

#[derive(Debug, thiserror::Error)]
pub enum Err {
    #[error("Reader error: {0}")]
    Reader(#[from] ReaderErr),
    #[error("Invalid attribute domain id")]
    InvalidAttributeDomain,
    #[error("Invalid attribute type id")]
    InvalidAttributeType,
    #[error("Invalid component data type id")]
    InvalidComponentDataType,
    #[error("Invalid traversal type id: {0}")]
    InvalidTraversalType(u8),
    #[error("Invalid portabilization type id: {0}")]
    InvalidPortabilizationType(u8),
    #[error("Per-attribute prediction scheme not yet implemented: id={0}")]
    PredictionSchemeTodo(u8),
    #[error("RANS encoding flag was {0}, expected 1")]
    RansEncodingDisabled(u8),
    #[error("Inverse prediction transform error: {0}")]
    InverseTransform(#[from] inverse_prediction_transform::Err),
    #[error("Deportabilization error: {0}")]
    Deportabilization(#[from] portabilization::Err),
    #[error("Symbol coding error: {0}")]
    SymbolCoding(#[from] symbol_coding::Err),
    #[error("Rans decoder error: {0}")]
    Rans(#[from] crate::decode::entropy::rans::Err),
    #[error("Unsupported component count: {0}")]
    UnsupportedNumComponents(u8),
    #[error("Symbol stream ran out mid-decode (corner sequence yielded more vertices than symbols)")]
    SymbolStreamUnderrun,
    #[error("Symbol stream had leftover symbols after decode (vertex count mismatch?)")]
    SymbolStreamSurplus,
    #[error("Attribute core error: {0}")]
    AttributeCore(#[from] crate::core::attribute::Err),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TraversalType {
    DepthFirst,
    PredictionDegree,
}

impl TraversalType {
    fn from_id(id: u8) -> Result<Self, Err> {
        match id {
            0 => Ok(Self::DepthFirst),
            1 => Ok(Self::PredictionDegree),
            _ => Err(Err::InvalidTraversalType(id)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PortabilizationType {
    ToBits,
    QuantizationCoordinateWise,
    OctahedralQuantization,
}

impl PortabilizationType {
    fn from_id(id: u8) -> Result<Self, Err> {
        // IDs match `encode/attribute/portabilization/mod.rs:84-92`.
        match id {
            1 => Ok(Self::ToBits),
            2 => Ok(Self::QuantizationCoordinateWise),
            3 => Ok(Self::OctahedralQuantization),
            _ => Err(Err::InvalidPortabilizationType(id)),
        }
    }
}

/// Per-attribute metadata read from the bitstream before the actual data.
#[allow(dead_code)] // Read during the per-attribute decode pipeline.
pub(crate) struct AttributeMeta {
    pub decoder_id: Option<u8>,
    pub domain: AttributeDomain,
    pub traversal: TraversalType,

    pub attribute_type: AttributeType,
    pub component_type: ComponentDataType,
    pub num_components: u8,
    pub normalized: u8,
    pub unique_id: u8,
    pub portabilization: PortabilizationType,
}

#[derive(Debug, Clone)]
pub struct Config {}

impl crate::prelude::ConfigType for Config {
    fn default() -> Self {
        Self {}
    }
}

/// Reads all attribute metadata then decodes each attribute.
///
/// When an unsupported attribute is encountered, decoding stops and
/// returns the attributes decoded so far so that downstream consumers
/// can still use the positions.
pub(crate) fn decode_attributes<R: ByteReader>(
    reader: &mut R,
    header: &Header,
    corner_table: &DecoderCornerTable,
    attribute_corner_tables: &[crate::decode::connectivity::DecoderAttributeCornerTable],
    start_corners: &[CornerIdx],
    num_position_vertices: usize,
    cfg: Config,
) -> Result<Vec<Attribute>, Err> {
    Ok(decode_attributes_with_meta(
        reader,
        header,
        corner_table,
        attribute_corner_tables,
        start_corners,
        num_position_vertices,
        cfg,
    )?
    .into_iter()
    .map(|(att, _)| att)
    .collect())
}

/// Like [`decode_attributes`] but also returns each attribute's
/// `decoder_id`. `None` means the attribute is decoded against the
/// universal corner table (position); `Some(idx)` indexes into
/// `attribute_corner_tables`.
pub(crate) fn decode_attributes_with_meta<R: ByteReader>(
    reader: &mut R,
    header: &Header,
    corner_table: &DecoderCornerTable,
    attribute_corner_tables: &[crate::decode::connectivity::DecoderAttributeCornerTable],
    start_corners: &[CornerIdx],
    num_position_vertices: usize,
    _cfg: Config,
) -> Result<Vec<(Attribute, Option<u8>)>, Err> {
    let metas = read_metadata(reader, header)?;
    let mut out: Vec<(Attribute, Option<u8>)> = Vec::with_capacity(metas.len());
    // Auxiliary buffer of QUANTIZED positions (i32, in the encoder's
    // [0, max_quant] range) INDEXED BY CORNER-TABLE VERTEX ID — not the
    // compacted attribute index. `MeshNormalPrediction` works in the
    // i32 quantized domain, so passing the original i32 values (rather
    // than dequantized f32 + re-quantize) avoids a precision-losing
    // round trip.
    let mut positions_by_ct_vertex: Option<Vec<[i32; 3]>> = None;
    for meta in &metas {
        let position_parent = out
            .iter()
            .map(|(a, _)| a)
            .find(|a| a.get_attribute_type() == AttributeType::Position);
        // Pick the corner table this attribute should be decoded against.
        // `decoder_id` indexes into `attribute_corner_tables` (encoder
        // wrote `(i as u8).wrapping_sub(1)`, so 0xFF = use universal
        // table for the first attribute = position).
        let attr_table = match meta.decoder_id {
            Some(idx) if (idx as usize) < attribute_corner_tables.len() => {
                Some(&attribute_corner_tables[idx as usize])
            }
            _ => None,
        };
        match decode_one_attribute(
            reader,
            meta,
            corner_table,
            attr_table,
            start_corners,
            num_position_vertices,
            position_parent,
            positions_by_ct_vertex.as_deref(),
        ) {
            Ok((att, ct_indexed, effective_decoder_id)) => {
                if att.get_attribute_type() == AttributeType::Position {
                    positions_by_ct_vertex = ct_indexed;
                }
                out.push((att, effective_decoder_id));
            }
            // Best-effort: if a non-position attribute trips an
            // unimplemented decode path (oct transforms, oct port,
            // 2-component layouts, MeshNormalPrediction, etc.), return
            // what we've decoded so far rather than failing the whole
            // mesh. The caller still gets correctly-decoded positions.
            // After this, the byte stream is in an undefined state — we
            // can't continue to the next attribute.
            Err(Err::PredictionSchemeTodo(_))
            | Err(Err::UnsupportedNumComponents(_))
            | Err(Err::InverseTransform(
                inverse_prediction_transform::Err::OctahedralTodo,
            )) => {
                return Ok(out);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// Decodes a single attribute into an `Attribute`, mirroring
/// `encode/attribute/attribute_encoder.rs::encode_portabilized` byte-for-byte
/// on the read side and applying the inverse pipeline:
///
///   symbols → from_positive_i32 → inverse_transform(corr, prediction)
///                              → quantized i32 (per vertex)
///                              → deportabilize → original f32 attribute
///
/// `start_corners` are the seeds for the per-attribute traverser (one per
/// connected component, recorded during `decode_connectivity`).
/// Returns `(attribute, optional_ct_vertex_indexed_positions,
/// effective_decoder_id)`. The second value is `Some` only for the
/// Position branch; downstream normal/UV decoders use it to look up
/// positions by corner-table vertex ID rather than the compacted
/// attribute index. The third value is the corner table actually used
/// for indexing this attribute's value buffer: `None` = universal
/// corner table, `Some(idx)` = `attribute_corner_tables[idx]`. Some
/// branches OVERRIDE the bitstream's `decoder_id` (e.g. UV's
/// MeshPredictionForTextureCoordinates currently always uses the
/// universal table), so callers that index back into the value
/// buffer (`decode_to_raw`) must use this effective ID rather than
/// `meta.decoder_id`.
fn decode_one_attribute<R: ByteReader>(
    reader: &mut R,
    meta: &AttributeMeta,
    corner_table: &DecoderCornerTable,
    attr_table: Option<&crate::decode::connectivity::DecoderAttributeCornerTable>,
    start_corners: &[CornerIdx],
    num_position_vertices: usize,
    position_parent: Option<&Attribute>,
    positions_by_ct_vertex: Option<&[[i32; 3]]>,
) -> Result<(Attribute, Option<Vec<[i32; 3]>>, Option<u8>), Err> {
    // ── 1-3: per-attribute header bytes ─────────────────────────────────
    let pred_scheme_id = reader.read_u8()?;
    let xform_kind = InverseTransformKind::from_id(reader.read_u8()?)?;
    let rans_encoding = reader.read_u8()?;
    if rans_encoding != 1 {
        return Err(Err::RansEncodingDisabled(rans_encoding));
    }

    let n_components = meta.num_components as usize;

    // Dispatch on (attribute_type, port_kind). The encoder bundles attr
    // type with a specific (prediction_scheme + transform + port) triple;
    // we mirror those triples here with one branch per real combination.
    match (meta.attribute_type, meta.portabilization) {
        // Position: N=3, MeshParallelogramPrediction + WrappedDifference
        // + QuantizationCoordinateWise. Verified compatible with Google
        // for tetrahedron/sphere/torus/bunny.
        (AttributeType::Position, PortabilizationType::QuantizationCoordinateWise)
            if n_components == 3 =>
        {
            let (att, ct_idx) = decode_quantized_attribute::<R, 3>(
                reader,
                meta,
                corner_table,
                num_position_vertices,
                pred_scheme_id,
                xform_kind,
                start_corners,
                /* return_ct_indexed */ true,
            )?;
            Ok((att, ct_idx, None))
        }
        // TextureCoordinate: N=2, MeshPredictionForTextureCoordinates (id=5)
        // + WrappedDifference + QuantizationCoordinateWise — the Google
        // default. The encoder side picks a 3D-triangle-plane prediction
        // (closest of two sign-flipped variants) and stores one
        // orientation bit per visited vertex.
        (AttributeType::TextureCoordinate, PortabilizationType::QuantizationCoordinateWise)
            if n_components == 2 && pred_scheme_id == MESH_PREDICTION_FOR_TEXTURE_COORDINATES_ID =>
        {
            // attr_table threading regresses our self-roundtrip (since
            // our encoder doesn't use attribute corner tables for
            // small meshes) and doesn't fully align with Google's
            // output either (~0.07-0.20 max L2 on Skyline-style
            // photogrammetry b3dms). Until both ends are reconciled,
            // pass None — gives bit-perfect against Google's output
            // for parallelogram-prediction UVs (3D Tiles
            // KHR_draco_mesh_compression at default cl), and
            // ~0.075 self-roundtrip on tetra.
            let _attr_table_unused = attr_table;
            let att = decode_uv_attribute(
                reader,
                meta,
                corner_table,
                None,
                start_corners,
                num_position_vertices,
                xform_kind,
                positions_by_ct_vertex,
            )?;
            // OVERRIDE: even though the bitstream said use
            // attribute_corner_tables[meta.decoder_id], we forced
            // None above. Tell callers to index via the universal
            // corner table.
            Ok((att, None, None))
        }
        // TextureCoordinate fallback for parallelogram-style predictions.
        (AttributeType::TextureCoordinate, PortabilizationType::QuantizationCoordinateWise)
            if n_components == 2 =>
        {
            let (att, _) = decode_quantized_attribute::<R, 2>(
                reader,
                meta,
                corner_table,
                num_position_vertices,
                pred_scheme_id,
                xform_kind,
                start_corners,
                false,
            )?;
            Ok((att, None, None))
        }
        // Normal: stored as N=2 oct-quantized i32 in the symbol stream
        // even though the metadata says num_components=3 (the output
        // dim — what the consumer of the Attribute sees). Output is 3D
        // unit normals.
        (AttributeType::Normal, PortabilizationType::OctahedralQuantization) => {
            let att = decode_normal_attribute(
                reader,
                meta,
                corner_table,
                attr_table,
                num_position_vertices,
                position_parent,
                positions_by_ct_vertex,
            )?;
            // Effective table = whatever we actually passed to the
            // decoder (Some when seamed, None when not).
            let effective = if attr_table.is_some() {
                meta.decoder_id
            } else {
                None
            };
            Ok((att, None, effective))
        }
        // Color: N=3 (RGB) or N=4 (RGBA), QuantizationCoordinateWise
        // with DeltaPrediction. Same decode path as Position, just
        // parameterized over N. The encoder side defaults Color to
        // QuantizationCoordinateWise + WrappedDifference + 11-bit.
        (AttributeType::Color, PortabilizationType::QuantizationCoordinateWise)
            if n_components == 3 =>
        {
            let (att, _) = decode_quantized_attribute::<R, 3>(
                reader,
                meta,
                corner_table,
                num_position_vertices,
                pred_scheme_id,
                xform_kind,
                start_corners,
                false,
            )?;
            Ok((att, None, None))
        }
        (AttributeType::Color, PortabilizationType::QuantizationCoordinateWise)
            if n_components == 4 =>
        {
            let (att, _) = decode_quantized_attribute::<R, 4>(
                reader,
                meta,
                corner_table,
                num_position_vertices,
                pred_scheme_id,
                xform_kind,
                start_corners,
                false,
            )?;
            Ok((att, None, None))
        }
        // Custom: N=3 with ToBits (passthrough). Same prediction flow as
        // Position but no dequantization to f32 — still produces f32
        // values for now (pass-through int-as-float).
        (_, PortabilizationType::ToBits) if n_components == 3 => {
            let (att, _) = decode_quantized_attribute::<R, 3>(
                reader,
                meta,
                corner_table,
                num_position_vertices,
                pred_scheme_id,
                xform_kind,
                start_corners,
                false,
            )?;
            Ok((att, None, None))
        }
        _ => Err(Err::UnsupportedNumComponents(meta.num_components)),
    }
}

/// Decode flow for N-component attributes that use:
/// `MeshParallelogramPrediction` (or fall-back to last-decoded) +
/// `WrappedDifference` + `QuantizationCoordinateWise`.
///
/// Used by Position (N=3) and TextureCoordinate (N=2). Both attribute
/// types share the same byte layout once you parameterize over N.
fn decode_quantized_attribute<R: ByteReader, const N: usize>(
    reader: &mut R,
    meta: &AttributeMeta,
    corner_table: &DecoderCornerTable,
    num_attr_values: usize,
    pred_scheme_id: u8,
    xform_kind: InverseTransformKind,
    _start_corners: &[CornerIdx],
    return_ct_indexed: bool,
) -> Result<(Attribute, Option<Vec<[i32; 3]>>), Err>
where
    NdVector<N, f32>: Vector<N, Component = f32>,
    NdVector<N, i32>: Vector<N, Component = i32>,
{
    let num_symbols = num_attr_values * N;
    let symbols = symbol_coding::decode_symbols(num_symbols, N, reader)?;

    if !is_simple_prediction_scheme(pred_scheme_id)
        && pred_scheme_id != MESH_PARALLELOGRAM_PREDICTION_ID
    {
        return Err(Err::PredictionSchemeTodo(pred_scheme_id));
    }
    let inverse_xform = InverseTransform::read(reader, xform_kind)?;
    let port_kind = match meta.portabilization {
        PortabilizationType::ToBits => DeportabilizationKind::ToBits,
        PortabilizationType::QuantizationCoordinateWise => {
            DeportabilizationKind::QuantizationCoordinateWise
        }
        PortabilizationType::OctahedralQuantization => {
            DeportabilizationKind::OctahedralQuantization
        }
    };
    let dequant = match port_kind {
        DeportabilizationKind::QuantizationCoordinateWise => Some(Quantization::read(reader, N)?),
        DeportabilizationKind::ToBits => None,
        DeportabilizationKind::OctahedralQuantization => unreachable!("not for this path"),
    };

    let num_faces = corner_table.num_faces();
    let mut seeds = Vec::with_capacity(num_faces);
    for i in (0..num_faces).rev() {
        seeds.push(CornerIdx::from(3 * i));
    }
    let traverser = Traverser::new(corner_table, seeds);
    let sequence = traverser.compute_seqeunce();

    let buf_len = corner_table.num_vertices().max(num_attr_values);
    let mut partial: Vec<[i32; N]> = vec![[0; N]; buf_len];
    let mut visited = vec![false; buf_len];
    let mut last_decoded: [i32; N] = [0; N];
    let mut symbol_idx = 0usize;

    for c in &sequence {
        let v = corner_table.vertex_idx(*c);
        let v_idx = usize::from(v);
        if visited[v_idx] {
            continue;
        }

        let pred = if pred_scheme_id == MESH_PARALLELOGRAM_PREDICTION_ID {
            predict_parallelogram_n::<N>(corner_table, *c, &visited, &partial, last_decoded)
        } else {
            last_decoded
        };

        if symbol_idx + N > symbols.len() {
            return Err(Err::SymbolStreamUnderrun);
        }
        let mut corr = [0i32; N];
        for i in 0..N {
            corr[i] = symbols[symbol_idx + i] as i32;
        }
        symbol_idx += N;

        let mut value = [0i32; N];
        inverse_xform.inverse_n(&corr, &pred, &mut value);
        partial[v_idx] = value;
        last_decoded = value;
        visited[v_idx] = true;
    }

    if symbol_idx != symbols.len() {
        return Err(Err::SymbolStreamSurplus);
    }

    // Deportabilize. Output vertices in vertex-id order, written
    // straight into the final `Vec<NdVector<N, f32>>` so we don't
    // pay for a flat `Vec<f32>` intermediate + element-wise repack.
    let mut data: Vec<NdVector<N, f32>> = Vec::with_capacity(num_attr_values);
    let mut tmp = vec![0f32; N];
    for (i, v) in partial.iter().enumerate() {
        if !visited[i] {
            continue;
        }
        match &dequant {
            Some(q) => q.dequantize_into(v, &mut tmp),
            None => {
                for j in 0..N {
                    tmp[j] = v[j] as f32;
                }
            }
        }
        let mut nd = <NdVector<N, f32> as Vector<N>>::zero();
        for j in 0..N {
            *nd.get_mut(j) = tmp[j];
        }
        data.push(nd);
    }

    // Optionally produce a vertex-indexed (= corner-table indexed)
    // copy of the QUANTIZED i32 positions for downstream Normal
    // prediction. The encoder's MeshNormalPrediction operates in the
    // i32 quantized domain, so passing through the integer values
    // directly avoids the precision loss of dequantize → re-quantize.
    // Only ever requested for N=3 Position attributes.
    let ct_indexed: Option<Vec<[i32; 3]>> = if return_ct_indexed && N == 3 {
        let mut out: Vec<[i32; 3]> = vec![[0; 3]; buf_len];
        for (i, v) in partial.iter().enumerate() {
            if visited[i] {
                let mut p = [0i32; 3];
                for j in 0..3 {
                    p[j] = v[j];
                }
                out[i] = p;
            }
        }
        Some(out)
    } else {
        None
    };

    // Use `from_without_removing_duplicates` — the decoder must
    // preserve the on-wire value ordering so per-vertex lookups via
    // attribute corner table indices stay correct. `Attribute::from`'s
    // dedup pass compacts the buffer and introduces a
    // `point_to_att_val_map` that downstream consumers (decode_to_raw,
    // splice_glb_remove_draco) don't navigate; meshes with duplicate
    // attribute values would otherwise scramble per-vertex lookups.
    let attr = Attribute::from_without_removing_duplicates(
        AttributeId::new(meta.unique_id as usize),
        data,
        meta.attribute_type,
        meta.domain,
        Vec::new(),
    );
    Ok((attr, ct_indexed))
}

/// N-component generic version of `predict_parallelogram`. Operates on
/// `[i32; N]` slots in `partial`.
fn predict_parallelogram_n<const N: usize>(
    ct: &DecoderCornerTable,
    c: CornerIdx,
    visited: &[bool],
    partial: &[[i32; N]],
    last_decoded: [i32; N],
) -> [i32; N] {
    let opp = match ct.opposite(c) {
        Some(o) => o,
        None => return last_decoded,
    };
    let opp_v = ct.vertex_idx(opp);
    let next_v = ct.vertex_idx(ct.next(c));
    let prev_v = ct.vertex_idx(ct.previous(c));

    let opp_vi = usize::from(opp_v);
    let next_vi = usize::from(next_v);
    let prev_vi = usize::from(prev_v);

    if !visited[opp_vi] || !visited[next_vi] || !visited[prev_vi] {
        return last_decoded;
    }

    let a = partial[next_vi];
    let b = partial[prev_vi];
    let diag = partial[opp_vi];

    let mut out = [0i32; N];
    for i in 0..N {
        out[i] = a[i] + b[i] - diag[i];
    }
    out
}

/// Decode flow for normal attributes: N=2 oct-quantized i32 values
/// produced by `MeshNormalPrediction` + `OctahedralOrthogonal` +
/// `OctahedralQuantization`. Output is N=3 unit-vector f32 normals.
///
/// `MeshNormalPrediction` requires positions (the parent attribute)
/// to compute predictions from the corner table; when those aren't
/// available we fall back to `last_decoded` prediction so the byte
/// stream is consumed correctly even though the produced values are
/// only quantized-coherent, not semantically correct.
fn decode_normal_attribute<R: ByteReader>(
    reader: &mut R,
    meta: &AttributeMeta,
    corner_table: &DecoderCornerTable,
    attr_table: Option<&crate::decode::connectivity::DecoderAttributeCornerTable>,
    num_position_vertices: usize,
    _position_parent: Option<&Attribute>,
    positions_by_ct_vertex: Option<&[[i32; 3]]>,
) -> Result<Attribute, Err> {
    use crate::core::bit_coder::BitReader;
    use crate::core::buffer::LsbFirst;
    use crate::decode::entropy::rans::RabsDecoder;

    const N: usize = 2;
    // When the encoder uses a per-attribute corner table for normals
    // (normal-seamed mesh), the attribute vertex count differs from
    // the position vertex count.
    let num_attr_values = attr_table
        .map(|t| t.num_vertices)
        .unwrap_or(num_position_vertices);
    let num_symbols = num_attr_values * N;
    let symbols = symbol_coding::decode_symbols(num_symbols, N, reader)?;

    let inverse_xform = OctahedralOrthogonalInverseTransform::read(reader)?;

    // MeshNormalPrediction writes its own metadata: u8 prob_zero +
    // leb128 len + RABS-coded flip bits (one per normal vertex).
    let flip_prob = reader.read_u8()?;
    let flip_buf_len = crate::utils::bit_coder::leb128_read(reader)? as usize;
    let flip_buf = crate::utils::bit_coder::read_byte_buffer(reader, flip_buf_len)?;
    let mut flips: Vec<bool> = Vec::with_capacity(num_attr_values);
    if flip_buf_len > 0 {
        let mut iter = flip_buf.into_iter();
        let mut rabs: RabsDecoder<_> =
            RabsDecoder::new(&mut iter, flip_buf_len, flip_prob as usize, None)?;
        for _ in 0..num_attr_values {
            flips.push(rabs.read().unwrap_or(0) != 0);
        }
    } else {
        flips.resize(num_attr_values, false);
    }
    let _ = BitReader::<'_, std::vec::IntoIter<u8>, LsbFirst>::spown_from;

    let dequant = OctahedralNormal::read(reader)?;

    // Helper closures: vertex_idx via the chosen corner table.
    let attr_v_idx = |c: CornerIdx| -> usize {
        match attr_table {
            Some(t) => usize::from(<crate::decode::connectivity::DecoderAttributeCornerTable as crate::core::corner_table::GenericCornerTable>::vertex_idx(t, c)),
            None => usize::from(corner_table.vertex_idx(c)),
        }
    };
    let universal_v_idx = |c: CornerIdx| -> usize { usize::from(corner_table.vertex_idx(c)) };

    let num_faces = corner_table.num_faces();
    let mut seeds = Vec::with_capacity(num_faces);
    for i in (0..num_faces).rev() {
        seeds.push(CornerIdx::from(3 * i));
    }
    let sequence = match attr_table {
        Some(t) => Traverser::new(t, seeds).compute_seqeunce(),
        None => Traverser::new(corner_table, seeds).compute_seqeunce(),
    };

    let buf_len = num_attr_values.max(corner_table.num_vertices());
    let mut partial: Vec<[i32; 2]> = vec![[0; 2]; buf_len];
    let mut visited = vec![false; buf_len];
    let mut symbol_idx = 0usize;
    let mut flip_idx = 0usize;

    for c in &sequence {
        let v_idx = attr_v_idx(*c);
        if visited[v_idx] {
            continue;
        }

        let pred = match positions_by_ct_vertex {
            Some(positions) => predict_normal(
                corner_table,
                attr_table,
                *c,
                positions,
                inverse_xform.center_value,
                inverse_xform.max_quantized_value,
                flips.get(flip_idx).copied().unwrap_or(false),
                universal_v_idx(*c),
            ),
            None => [inverse_xform.center_value, inverse_xform.center_value],
        };
        flip_idx += 1;

        if symbol_idx + N > symbols.len() {
            return Err(Err::SymbolStreamUnderrun);
        }
        let corr = [symbols[symbol_idx] as i32, symbols[symbol_idx + 1] as i32];
        symbol_idx += N;

        let value = inverse_xform.inverse(&corr, &pred);
        partial[v_idx] = value;
        visited[v_idx] = true;
    }

    if symbol_idx != symbols.len() {
        return Err(Err::SymbolStreamSurplus);
    }

    // Dequantize 2D oct → 3D unit normal.
    let mut data: Vec<NdVector<3, f32>> = Vec::with_capacity(num_attr_values);
    for (i, v) in partial.iter().enumerate() {
        if !visited[i] {
            continue;
        }
        let n = dequant.dequantize(v);
        let mut nd = <NdVector<3, f32> as Vector<3>>::zero();
        *nd.get_mut(0) = n[0];
        *nd.get_mut(1) = n[1];
        *nd.get_mut(2) = n[2];
        data.push(nd);
    }

    // Use `from_without_removing_duplicates` — the decoder must
    // preserve the on-wire value ordering so per-vertex lookups via
    // attribute corner table indices stay correct. `Attribute::from`'s
    // dedup pass compacts the buffer and introduces a
    // `point_to_att_val_map` that downstream consumers (decode_to_raw,
    // splice_glb_remove_draco) don't navigate; meshes with duplicate
    // attribute values would otherwise scramble per-vertex lookups.
    let attr = Attribute::from_without_removing_duplicates(
        AttributeId::new(meta.unique_id as usize),
        data,
        meta.attribute_type,
        meta.domain,
        Vec::new(),
    );
    Ok(attr)
}

/// Inverse `MeshNormalPrediction`. Mirrors
/// `shared/attribute/prediction_scheme/mesh_normal_prediction.rs::predict`:
///   1. Sum face normals around vertex of corner `c` (cross products of
///      neighbour-position deltas).
///   2. Cast down + apply `octahedral_transform` to project onto the
///      octahedron face.
///   3. Scale to oct-quantized i32.
///   4. If the encoder's flip bit is set, negate the result.
fn predict_normal(
    ct: &DecoderCornerTable,
    attr_table: Option<&crate::decode::connectivity::DecoderAttributeCornerTable>,
    c: CornerIdx,
    positions_by_ct_vertex: &[[i32; 3]],
    center_value: i32,
    max_quantized_value: i32,
    flip: bool,
    universal_v_idx: usize,
) -> [i32; 2] {
    use crate::core::corner_table::GenericCornerTable;
    let pos_c = positions_by_ct_vertex
        .get(universal_v_idx)
        .copied()
        .unwrap_or([0; 3]);

    // Walk to leftmost adjacent corner. When attr_table is set, use
    // its swing (which respects seam edges as boundaries) so we sum
    // face normals only over the FAN that belongs to this attribute
    // vertex. Otherwise, walk the full universal 1-ring.
    let swing_left = |curr: CornerIdx| -> Option<CornerIdx> {
        match attr_table {
            Some(t) => GenericCornerTable::swing_left(t, curr),
            None => ct.swing_left(curr),
        }
    };
    let swing_right = |curr: CornerIdx| -> Option<CornerIdx> {
        match attr_table {
            Some(t) => GenericCornerTable::swing_right(t, curr),
            None => ct.swing_right(curr),
        }
    };

    // Mirror Google's VertexCornersIterator: emit corner_id first,
    // walk SwingLeft until boundary, then SwingRight from start_corner
    // until another boundary. Avoids the "swing-left-to-leftmost,
    // then swing-right-from-there" indirection — semantically the
    // same SET of corners but with explicit boundary handling.
    let mut sum: [i64; 3] = face_normal_i64(ct, c, pos_c, positions_by_ct_vertex);
    {
        let mut curr = c;
        loop {
            match swing_left(curr) {
                Some(next) if next == c => break,
                Some(next) => {
                    curr = next;
                    let f = face_normal_i64(ct, curr, pos_c, positions_by_ct_vertex);
                    for k in 0..3 {
                        sum[k] += f[k];
                    }
                }
                None => {
                    // Boundary — switch to swinging right from `c`.
                    let mut r = c;
                    while let Some(rn) = swing_right(r) {
                        r = rn;
                        let f = face_normal_i64(ct, r, pos_c, positions_by_ct_vertex);
                        for k in 0..3 {
                            sum[k] += f[k];
                        }
                    }
                    break;
                }
            }
        }
    }

    // Cap |sum| ≤ 2^29 to keep i32 conversions safe (mirrors Google's
    // GeometricNormalPredictorArea::ComputePredictedValue cap).
    let upper_bound: i64 = 1 << 29;
    let abs = sum[0].abs() + sum[1].abs() + sum[2].abs();
    if abs > upper_bound {
        let q = abs / upper_bound;
        if q > 0 {
            for k in 0..3 {
                sum[k] /= q;
            }
        }
    }

    let mut vec3 = [sum[0] as i32, sum[1] as i32, sum[2] as i32];

    // CanonicalizeIntegerVector: project onto the octahedron surface
    // such that |v[0]| + |v[1]| + |v[2]| == center_value. Mirrors
    // Google's OctahedronToolBox::CanonicalizeIntegerVector.
    let abs_sum = (vec3[0].abs() as i64) + (vec3[1].abs() as i64) + (vec3[2].abs() as i64);
    if abs_sum == 0 {
        vec3[0] = center_value;
    } else {
        vec3[0] = (((vec3[0] as i64) * (center_value as i64)) / abs_sum) as i32;
        vec3[1] = (((vec3[1] as i64) * (center_value as i64)) / abs_sum) as i32;
        if vec3[2] >= 0 {
            vec3[2] = center_value - vec3[0].abs() - vec3[1].abs();
        } else {
            vec3[2] = -(center_value - vec3[0].abs() - vec3[1].abs());
        }
    }

    // Flip in 3D BEFORE oct conversion (Google does
    // `pred_normal_3d = -pred_normal_3d` on the canonicalized 3D
    // normal, then converts to oct).
    if flip {
        vec3[0] = -vec3[0];
        vec3[1] = -vec3[1];
        vec3[2] = -vec3[2];
    }

    // IntegerVectorToQuantizedOctahedralCoords + CanonicalizeOctahedralCoords.
    integer_vector_to_quantized_oct(vec3, center_value, max_quantized_value)
}

/// Mirror of Google's
/// `OctahedronToolBox::IntegerVectorToQuantizedOctahedralCoords` +
/// `CanonicalizeOctahedralCoords` (`normal_compression_utils.h`).
fn integer_vector_to_quantized_oct(
    int_vec: [i32; 3],
    center_value: i32,
    max_value: i32,
) -> [i32; 2] {
    let mut s;
    let mut t;
    if int_vec[0] >= 0 {
        // Right hemisphere.
        s = int_vec[1] + center_value;
        t = int_vec[2] + center_value;
    } else {
        // Left hemisphere.
        s = if int_vec[1] < 0 {
            int_vec[2].abs()
        } else {
            max_value - int_vec[2].abs()
        };
        t = if int_vec[2] < 0 {
            int_vec[1].abs()
        } else {
            max_value - int_vec[1].abs()
        };
    }
    // CanonicalizeOctahedralCoords: snap edge points to canonical positions.
    if (s == 0 && t == 0) || (s == 0 && t == max_value) || (s == max_value && t == 0) {
        s = max_value;
        t = max_value;
    } else if s == 0 && t > center_value {
        t = center_value - (t - center_value);
    } else if s == max_value && t < center_value {
        t = center_value + (center_value - t);
    } else if t == max_value && s < center_value {
        s = center_value + (center_value - s);
    } else if t == 0 && s > center_value {
        s = center_value - (s - center_value);
    }
    [s, t]
}

fn face_normal_i64(
    ct: &DecoderCornerTable,
    c: CornerIdx,
    pos_c: [i32; 3],
    positions_by_ct_vertex: &[[i32; 3]],
) -> [i64; 3] {
    let next_vi = usize::from(ct.vertex_idx(ct.next(c)));
    let prev_vi = usize::from(ct.vertex_idx(ct.previous(c)));
    let pn = positions_by_ct_vertex.get(next_vi).copied().unwrap_or([0; 3]);
    let pp = positions_by_ct_vertex.get(prev_vi).copied().unwrap_or([0; 3]);
    let dn = [pn[0] - pos_c[0], pn[1] - pos_c[1], pn[2] - pos_c[2]];
    let dp = [pp[0] - pos_c[0], pp[1] - pos_c[1], pp[2] - pos_c[2]];
    [
        (dn[1] as i64) * (dp[2] as i64) - (dn[2] as i64) * (dp[1] as i64),
        (dn[2] as i64) * (dp[0] as i64) - (dn[0] as i64) * (dp[2] as i64),
        (dn[0] as i64) * (dp[1] as i64) - (dn[1] as i64) * (dp[0] as i64),
    ]
}

/// Mirrors `encode/attribute/prediction_transform/geom.rs::into_faithful_oct_quantization`.
#[allow(dead_code)] // Float-path oct quantization helper, kept for reference vs the int port.
fn into_faithful_oct_quantization(vec: [i32; 2], max: i32) -> [i32; 2] {
    let half = max / 2;
    let u = vec[0];
    let v = vec[1];
    let mut x = u;
    let mut y = v;
    if (u == max || u == 0) && v == 0 || (u == 0 && v == max) {
        return [max, max];
    } else if u == 0 && v > half {
        y = half - (v - half);
    } else if u == max && v < half {
        y = half + (half - v);
    } else if v == max && u < half {
        x = half + (half - u);
    } else if v == 0 && u > half {
        x = half - (u - half);
    }
    [x, y]
}

/// Decode flow for TextureCoordinate attributes that use
/// `MeshPredictionForTextureCoordinates` + `WrappedDifference` +
/// `QuantizationCoordinateWise`. Mirrors
/// `shared/attribute/prediction_scheme/mesh_prediction_for_texture_coordinates.rs`.
///
/// Byte layout this consumes (after the 3 header bytes already read):
///   1. RANS-coded UV symbols (2 per visited vertex).
///   2. Prediction metadata (this scheme):
///        u32 — orientation bit count (one bit per complex prediction).
///        u8  — RABS zero_prob.
///        leb128 — RABS buffer length.
///        bytes  — RABS-coded RLE bits (encoder reverses them then runs
///                  `o == last_orientation ? 1 : 0`).
///   3. WrappedDifference transform metadata (min, max).
///   4. QuantizationCoordinateWise deportabilization metadata.
fn decode_uv_attribute<R: ByteReader>(
    reader: &mut R,
    meta: &AttributeMeta,
    corner_table: &DecoderCornerTable,
    attr_table: Option<&crate::decode::connectivity::DecoderAttributeCornerTable>,
    start_corners: &[CornerIdx],
    num_position_vertices: usize,
    xform_kind: InverseTransformKind,
    positions_by_ct_vertex: Option<&[[i32; 3]]>,
) -> Result<Attribute, Err> {
    use crate::decode::entropy::rans::RabsDecoder;

    const N: usize = 2;
    // The symbol count = number of *attribute* vertices times N. When
    // attr_table is present, that's the seam-split UV vertex count; else
    // fall back to the position vertex count (no UV seams).
    let num_attr_values = attr_table
        .map(|t| t.num_vertices)
        .unwrap_or(num_position_vertices);
    let num_symbols = num_attr_values * N;
    let symbols = symbol_coding::decode_symbols(num_symbols, N, reader)?;

    let orientation_count = {
        let b0 = reader.read_u8()?;
        let b1 = reader.read_u8()?;
        let b2 = reader.read_u8()?;
        let b3 = reader.read_u8()?;
        u32::from_le_bytes([b0, b1, b2, b3]) as usize
    };
    let flip_prob = reader.read_u8()?;
    let buf_len = crate::utils::bit_coder::leb128_read(reader)? as usize;
    let rabs_buf = crate::utils::bit_coder::read_byte_buffer(reader, buf_len)?;

    let inverse_xform = InverseTransform::read(reader, xform_kind)?;
    let dequant = Quantization::read(reader, N)?;

    // Mirror Google's RAnsBitDecoder semantics. Google's encoder calls
    // EncodeBit in forward order over orientations[0..N-1], its
    // EndEncoding reverses bits before rabs_write so RABS's LIFO read
    // brings them back to forward order in the decoder. Decoder reads
    // bits in EncodeBit order, applies forward delta-RLE to populate
    // orientations[0..N-1]. (See Google's
    // mesh_prediction_scheme_tex_coords_portable_decoder.h::DecodePredictionData
    // and rans_bit_encoder.cc::EndEncoding.)
    let mut bits = Vec::with_capacity(orientation_count);
    if buf_len > 0 && orientation_count > 0 {
        let mut iter = rabs_buf.into_iter();
        let mut rabs: RabsDecoder<_> =
            RabsDecoder::new(&mut iter, buf_len, flip_prob as usize, None)?;
        for _ in 0..orientation_count {
            bits.push(rabs.read().unwrap_or(0) != 0);
        }
    }
    let mut last = true;
    let mut orientations = Vec::with_capacity(orientation_count);
    for &b in &bits {
        if !b {
            last = !last;
        }
        orientations.push(last);
    }

    // Helper: pick the right corner-table vertex_idx for storage. When
    // attr_table is set, `v_idx` indexes attribute slots (post-seam-
    // split). Otherwise universal vertex IDs.
    let attr_v_idx = |c: CornerIdx| -> usize {
        match attr_table {
            Some(t) => usize::from(<crate::decode::connectivity::DecoderAttributeCornerTable as crate::core::corner_table::GenericCornerTable>::vertex_idx(t, c)),
            None => usize::from(corner_table.vertex_idx(c)),
        }
    };
    let universal_v_idx = |c: CornerIdx| -> usize { usize::from(corner_table.vertex_idx(c)) };

    // Set up traversal seeds. When attr_table is set, use the
    // start_corners (per-component start corners from edgebreaker
    // start-face replay) — these match what the encoder uses
    // (`corners_of_edgebreaker`). When attr_table is None, use face
    // seeds (one per face) since we're walking the universal corner
    // table and need to reach all faces.
    let sequence = match attr_table {
        Some(t) => {
            let seeds: Vec<CornerIdx> = start_corners.iter().copied().collect();
            Traverser::new(t, seeds).compute_seqeunce()
        }
        None => {
            let num_faces = corner_table.num_faces();
            let mut seeds = Vec::with_capacity(num_faces);
            for i in (0..num_faces).rev() {
                seeds.push(CornerIdx::from(3 * i));
            }
            Traverser::new(corner_table, seeds).compute_seqeunce()
        }
    };

    let buf_len = num_attr_values.max(corner_table.num_vertices());
    let mut partial: Vec<[i32; 2]> = vec![[0; 2]; buf_len];
    let mut visited = vec![false; buf_len];
    let mut symbol_idx = 0usize;
    // Google's decoder pops orientations from the BACK during the
    // forward iteration of corners. This is because the encoder
    // iterates p = N-1..0 (reverse), so the i-th complex-prediction
    // call in DECODER's forward iteration corresponds to the
    // i-th-FROM-END encoder push.
    let mut orientations_remaining = orientations;
    let mut last_decoded: [i32; 2] = [0; 2];

    for c in &sequence {
        let v_idx = attr_v_idx(*c);
        if visited[v_idx] {
            continue;
        }

        let next_c = corner_table.next(*c);
        let prev_c = corner_table.previous(*c);
        let next_vi = attr_v_idx(next_c);
        let prev_vi = attr_v_idx(prev_c);

        let pred = if visited[next_vi] && visited[prev_vi] {
            // Encoder: when both neighbors visited AND UVs equal,
            // returns prev_uv directly (no orientation push). Mirror
            // that — falling through to fallback would give a different
            // value AND mess up the orientation index.
            if partial[next_vi] == partial[prev_vi] {
                partial[prev_vi]
            } else {
            // Complex prediction path. Encoder pushes one orientation
            // here. Compute both candidate UVs and pick. Position
            // lookups use UNIVERSAL vertex IDs (not attribute).
            let pred_pair = uv_predict_complex(
                partial[v_idx],
                partial[next_vi],
                partial[prev_vi],
                positions_by_ct_vertex,
                universal_v_idx(*c),
                universal_v_idx(next_c),
                universal_v_idx(prev_c),
            );
            match pred_pair {
                Some((p0, p1)) => {
                    let orient = orientations_remaining.pop().unwrap_or(true);
                    if orient { p0 } else { p1 }
                }
                // Encoder hit overflow guard → fallback (no orientation
                // pushed). Decoder must also fall back.
                None => uv_predict_fallback_attr(
                    *c,
                    next_vi,
                    &visited,
                    &partial,
                    last_decoded,
                ),
            }
            }
        } else {
            uv_predict_fallback_attr(
                *c,
                next_vi,
                &visited,
                &partial,
                last_decoded,
            )
        };

        if symbol_idx + N > symbols.len() {
            return Err(Err::SymbolStreamUnderrun);
        }
        let mut corr = [0i32; N];
        for i in 0..N {
            corr[i] = symbols[symbol_idx + i] as i32;
        }
        symbol_idx += N;

        let mut value = [0i32; N];
        inverse_xform.inverse_n(&corr, &pred, &mut value);
        partial[v_idx] = value;
        last_decoded = value;
        visited[v_idx] = true;
    }

    if symbol_idx != symbols.len() {
        return Err(Err::SymbolStreamSurplus);
    }

    // Dequantize to f32 in attribute-vertex-id-ascending order,
    // straight into the final `Vec<NdVector<N, f32>>`.
    let mut data: Vec<NdVector<N, f32>> = Vec::with_capacity(num_attr_values);
    let mut tmp = vec![0f32; N];
    for (i, v) in partial.iter().enumerate() {
        if !visited[i] {
            continue;
        }
        dequant.dequantize_into(v, &mut tmp);
        let mut nd = <NdVector<N, f32> as Vector<N>>::zero();
        for j in 0..N {
            *nd.get_mut(j) = tmp[j];
        }
        data.push(nd);
    }
    // Use `from_without_removing_duplicates` — the decoder must
    // preserve the on-wire value ordering so per-vertex lookups via
    // attribute corner table indices stay correct. `Attribute::from`'s
    // dedup pass compacts the buffer and introduces a
    // `point_to_att_val_map` that downstream consumers (decode_to_raw,
    // splice_glb_remove_draco) don't navigate; meshes with duplicate
    // attribute values would otherwise scramble per-vertex lookups.
    let attr = Attribute::from_without_removing_duplicates(
        AttributeId::new(meta.unique_id as usize),
        data,
        meta.attribute_type,
        meta.domain,
        Vec::new(),
    );
    Ok(attr)
}

/// Inverse of `MeshPredictionForTextureCoordinates::predict` complex path.
/// Returns `Some((predicted_uv_0, predicted_uv_1))` matching the two
/// orientation choices the encoder considers, or `None` if any of the
/// encoder-side overflow guards trips (in which case the encoder
/// fell back without pushing an orientation).
fn uv_predict_complex(
    _curr_uv_unused: [i32; 2],
    next_uv_i32: [i32; 2],
    prev_uv_i32: [i32; 2],
    positions_by_ct_vertex: Option<&[[i32; 3]]>,
    curr_vi: usize,
    next_vi: usize,
    prev_vi: usize,
) -> Option<([i32; 2], [i32; 2])> {
    let positions = positions_by_ct_vertex?;
    let curr_pos = positions.get(curr_vi).copied()?;
    let next_pos = positions.get(next_vi).copied()?;
    let prev_pos = positions.get(prev_vi).copied()?;
    let curr_pos = [curr_pos[0] as i64, curr_pos[1] as i64, curr_pos[2] as i64];
    let next_pos = [next_pos[0] as i64, next_pos[1] as i64, next_pos[2] as i64];
    let prev_pos = [prev_pos[0] as i64, prev_pos[1] as i64, prev_pos[2] as i64];
    let next_uv = [next_uv_i32[0] as i64, next_uv_i32[1] as i64];
    let prev_uv = [prev_uv_i32[0] as i64, prev_uv_i32[1] as i64];

    let pn = [prev_pos[0] - next_pos[0], prev_pos[1] - next_pos[1], prev_pos[2] - next_pos[2]];
    let pn_norm2_squared = (pn[0] * pn[0] + pn[1] * pn[1] + pn[2] * pn[2]) as u64;
    if pn_norm2_squared == 0 {
        return None;
    }
    let cn = [curr_pos[0] - next_pos[0], curr_pos[1] - next_pos[1], curr_pos[2] - next_pos[2]];
    let cn_dot_pn = pn[0] * cn[0] + pn[1] * cn[1] + pn[2] * cn[2];
    let pn_uv = [prev_uv[0] - next_uv[0], prev_uv[1] - next_uv[1]];

    // Match encoder overflow guards.
    let n_uv_absmax = next_uv[0].abs().max(next_uv[1].abs());
    if pn_norm2_squared as i64 != 0 && n_uv_absmax > i64::MAX / pn_norm2_squared as i64 {
        return None;
    }
    let pn_uv_absmax = pn_uv[0].abs().max(pn_uv[1].abs());
    if pn_uv_absmax != 0 && cn_dot_pn.abs() > i64::MAX / pn_uv_absmax {
        return None;
    }

    let x_uv = [
        next_uv[0] * pn_norm2_squared as i64 + pn_uv[0] * cn_dot_pn,
        next_uv[1] * pn_norm2_squared as i64 + pn_uv[1] * cn_dot_pn,
    ];

    let pn_absmax = pn[0].abs().max(pn[1].abs()).max(pn[2].abs());
    if pn_absmax != 0 && cn_dot_pn.abs() > i64::MAX / pn_absmax {
        return None;
    }
    let pn_norm2_i = pn_norm2_squared as i64;
    // Encoder: `next_pos + pn * cn_dot_pn / pn_norm2_squared` which
    // evaluates as element-wise `(pn[k] * cn_dot_pn) / pn_norm2`.
    let x_pos = [
        next_pos[0] + (pn[0] * cn_dot_pn) / pn_norm2_i,
        next_pos[1] + (pn[1] * cn_dot_pn) / pn_norm2_i,
        next_pos[2] + (pn[2] * cn_dot_pn) / pn_norm2_i,
    ];
    let cx = [curr_pos[0] - x_pos[0], curr_pos[1] - x_pos[1], curr_pos[2] - x_pos[2]];
    let cx_norm2_squared = (cx[0] * cx[0] + cx[1] * cx[1] + cx[2] * cx[2]) as u64;

    let mut cx_uv = [pn_uv[1], -pn_uv[0]];
    let prod = cx_norm2_squared.checked_mul(pn_norm2_squared)?;
    let norm_squared = int_sqrt_u64(prod) as i64;
    cx_uv[0] *= norm_squared;
    cx_uv[1] *= norm_squared;

    let p0 = [
        ((x_uv[0] + cx_uv[0]) / pn_norm2_i) as i32,
        ((x_uv[1] + cx_uv[1]) / pn_norm2_i) as i32,
    ];
    let p1 = [
        ((x_uv[0] - cx_uv[0]) / pn_norm2_i) as i32,
        ((x_uv[1] - cx_uv[1]) / pn_norm2_i) as i32,
    ];
    Some((p0, p1))
}

/// Mirrors `MeshPredictionForTextureCoordinates::fallback_predict`.
/// Variant that takes pre-resolved attribute vertex indices so the
/// caller can use either the universal or attribute corner table for
/// vertex lookup.
fn uv_predict_fallback_attr(
    _c: CornerIdx,
    next_vi: usize,
    visited: &[bool],
    partial: &[[i32; 2]],
    last_decoded: [i32; 2],
) -> [i32; 2] {
    if next_vi < visited.len() && visited[next_vi] {
        return partial[next_vi];
    }
    last_decoded
}

/// Integer square root, mirroring
/// `MeshPredictionForTextureCoordinates::int_sqrt`. Newton's method.
fn int_sqrt_u64(value: u64) -> u64 {
    if value == 0 {
        return 0;
    }
    let mut act_number = value;
    let mut sqrt: u64 = 1;
    while act_number >= 2 {
        sqrt = sqrt.saturating_mul(2);
        act_number /= 4;
    }
    sqrt = (sqrt + value / sqrt) / 2;
    while sqrt.saturating_mul(sqrt) > value {
        sqrt = (sqrt + value / sqrt) / 2;
    }
    sqrt
}

/// On-wire IDs that
/// `shared/attribute/prediction_scheme/mod.rs::PredictionSchemeType::get_id`
/// writes:
///   0   → DeltaPrediction
///   1   → MeshParallelogramPrediction (Position default)
///   2   → MeshMultiParallelogramPrediction
///   5   → MeshPredictionForTextureCoordinates
///   6   → MeshNormalPrediction
///   7   → DerivativePrediction
///   0xFE → NoPrediction
const DELTA_PREDICTION_ID: u8 = 0;
const MESH_PARALLELOGRAM_PREDICTION_ID: u8 = 1;
const MESH_PREDICTION_FOR_TEXTURE_COORDINATES_ID: u8 = 5;
const NO_PREDICTION_ID: u8 = 0xFE;

fn is_simple_prediction_scheme(id: u8) -> bool {
    matches!(
        id,
        MESH_PARALLELOGRAM_PREDICTION_ID | DELTA_PREDICTION_ID | NO_PREDICTION_ID
    )
}

/// Reads only the per-attribute metadata block (steps 1-3 of the byte
/// layout). Useful for diagnostics / smoke tests before the full decode
/// pipeline lands.
pub(crate) fn read_metadata<R: ByteReader>(
    reader: &mut R,
    header: &Header,
) -> Result<Vec<AttributeMeta>, Err> {
    let num_attrs = reader.read_u8()? as usize;

    let mut decoder_ids: Vec<Option<u8>> = vec![None; num_attrs];
    let mut domains: Vec<AttributeDomain> = Vec::with_capacity(num_attrs);
    let mut traversals: Vec<TraversalType> = Vec::with_capacity(num_attrs);

    if header.encoding_method == EncoderMethod::Edgebreaker {
        for slot in decoder_ids.iter_mut().take(num_attrs) {
            *slot = Some(reader.read_u8()?);
            domains.push(
                AttributeDomain::read_from(reader)
                    .map_err(|_| Err::InvalidAttributeDomain)?,
            );
            traversals.push(TraversalType::from_id(reader.read_u8()?)?);
        }
    } else {
        // Sequential: encoder writes nothing here. Defaults are fine.
        for _ in 0..num_attrs {
            domains.push(AttributeDomain::Position);
            traversals.push(TraversalType::DepthFirst);
        }
    }

    let mut metas = Vec::with_capacity(num_attrs);
    for i in 0..num_attrs {
        let _count = reader.read_u8()?; // always 1 in current encoder
        let attribute_type =
            AttributeType::read_from(reader).map_err(|_| Err::InvalidAttributeType)?;
        let component_type =
            ComponentDataType::read_from(reader).map_err(|_| Err::InvalidComponentDataType)?;
        let num_components = reader.read_u8()?;
        let normalized = reader.read_u8()?;
        let unique_id = reader.read_u8()?;
        let portabilization = PortabilizationType::from_id(reader.read_u8()?)?;

        metas.push(AttributeMeta {
            decoder_id: decoder_ids[i],
            domain: domains[i],
            traversal: traversals[i],
            attribute_type,
            component_type,
            num_components,
            normalized,
            unique_id,
            portabilization,
        });
    }

    Ok(metas)
}
