//! glTF / GLB Draco decoder.
//!
//! Reads a `.glb` buffer (or pre-extracted JSON + binary buffer), finds
//! every primitive with the `KHR_draco_mesh_compression` extension,
//! extracts the Draco-compressed bufferView bytes, and decodes them via
//! [`crate::decode::decode`]. Returns one [`crate::core::mesh::Mesh`]
//! per Draco-compressed primitive, in the order they appear in the glTF
//! `meshes[*].primitives[*]` traversal.

use serde_json::{json, Value};

use crate::core::attribute::ComponentDataType;
use crate::core::mesh::Mesh;
use crate::core::shared::ConfigType;
use crate::decode;
use crate::io::gltf::{draco_extension, glb};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GLB parse error: {0}")]
    Glb(#[from] glb::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Decode error: {0}")]
    Decode(#[from] decode::Err),
    #[error("Missing field {0} on JSON path {1}")]
    MissingField(&'static str, String),
    #[error("BufferView {0} extends past the end of the binary buffer")]
    BufferViewOutOfRange(usize),
    #[error("BufferView {0} references buffer {1} but only buffer 0 (the GLB chunk) is supported")]
    NonZeroBuffer(usize, usize),
}

/// One decoded primitive with its provenance for routing back into the
/// glTF graph.
#[derive(Debug)]
pub struct DecodedPrimitive {
    /// Index of the source mesh in `glTF.meshes`.
    pub mesh_idx: usize,
    /// Index of the primitive within `glTF.meshes[mesh_idx].primitives`.
    pub primitive_idx: usize,
    /// The decoded geometry. Position is always present; normals + UVs
    /// are best-effort (see decoder graceful-fallback notes).
    pub mesh: Mesh,
}

/// Decode every Draco-compressed primitive in the GLB.
pub fn decode_glb(input: &[u8]) -> Result<Vec<DecodedPrimitive>, Error> {
    let glb_data = glb::parse_glb(input)?;
    let json: Value = serde_json::from_slice(&glb_data.json)?;
    decode_with_buffer(&json, &glb_data.buffer)
}

/// Decode every Draco-compressed primitive given pre-parsed glTF JSON
/// + the binary buffer (bufferView byte source). Useful when the caller
/// already has the GLB unpacked or when reading from non-GLB sources.
pub fn decode_with_buffer(json: &Value, binary_buffer: &[u8]) -> Result<Vec<DecodedPrimitive>, Error> {
    let mut out = Vec::new();
    let meshes = match json.get("meshes").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return Ok(out),
    };

    for (mesh_idx, mesh) in meshes.iter().enumerate() {
        let prims = match mesh.get("primitives").and_then(|p| p.as_array()) {
            Some(p) => p,
            None => continue,
        };
        for (primitive_idx, primitive) in prims.iter().enumerate() {
            if !draco_extension::is_draco_compressed(primitive) {
                continue;
            }
            let buffer_view_idx = primitive
                .get("extensions")
                .and_then(|e| e.get(draco_extension::EXTENSION_NAME))
                .and_then(|d| d.get("bufferView"))
                .and_then(|v| v.as_u64())
                .ok_or(Error::MissingField(
                    "bufferView",
                    format!("meshes[{}].primitives[{}].extensions.{}", mesh_idx, primitive_idx, draco_extension::EXTENSION_NAME),
                ))? as usize;

            let bytes = extract_buffer_view(json, binary_buffer, buffer_view_idx)?;
            let mut reader = bytes.into_iter();
            let mesh = decode::decode(&mut reader, decode::Config::default())?;
            out.push(DecodedPrimitive {
                mesh_idx,
                primitive_idx,
                mesh,
            });
        }
    }

    Ok(out)
}

/// Take a Draco-bearing GLB and return a Draco-free GLB ready for any
/// vanilla glTF loader (bevy_gltf, three.js, gltf-rs, ...). Decompress
/// every primitive, splice the decoded buffers back into the BIN as
/// plain bufferViews, patch the accessors that the primitives already
/// point at (count + componentType + bufferView + byteOffset), drop
/// the extension reference. Pass-through for GLBs that don't use the
/// extension (returns the input bytes verbatim).
pub fn splice_glb_remove_draco(input: &[u8]) -> Result<Vec<u8>, Error> {
    let glb_data = glb::parse_glb(input)?;
    let mut json: Value = serde_json::from_slice(&glb_data.json)?;

    if !json_has_draco_primitive(&json) {
        return Ok(input.to_vec());
    }

    let bin_bytes = &glb_data.buffer;

    let mut new_bin: Vec<u8> = bin_bytes.to_vec();
    align_to_4(&mut new_bin);

    let mut buffer_views = json
        .get("bufferViews")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut accessors = json
        .get("accessors")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let target_buffer_index: usize = 0;

    let meshes_owned = json
        .get("meshes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut meshes_new: Vec<Value> = Vec::with_capacity(meshes_owned.len());

    for mut mesh in meshes_owned.into_iter() {
        let prims_owned = mesh
            .get("primitives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut prims_new: Vec<Value> = Vec::with_capacity(prims_owned.len());

        for mut prim in prims_owned.into_iter() {
            let draco_ext = prim
                .get("extensions")
                .and_then(|e| e.get(draco_extension::EXTENSION_NAME))
                .cloned();
            let Some(ext) = draco_ext else {
                prims_new.push(prim);
                continue;
            };

            let old_index_accessor = prim
                .get("indices")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let old_attr_accessors: std::collections::BTreeMap<String, usize> = prim
                .get("attributes")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_u64().map(|u| (k.clone(), u as usize)))
                        .collect()
                })
                .unwrap_or_default();

            let ext_bv_index = ext
                .get("bufferView")
                .and_then(|v| v.as_u64())
                .ok_or(Error::MissingField(
                    "bufferView",
                    format!("primitive.extensions.{}", draco_extension::EXTENSION_NAME),
                ))? as usize;
            let (encoded_offset, encoded_len) = read_buffer_view_range(&buffer_views, ext_bv_index)?;
            if encoded_offset + encoded_len > bin_bytes.len() {
                return Err(Error::BufferViewOutOfRange(ext_bv_index));
            }
            let encoded = &bin_bytes[encoded_offset..encoded_offset + encoded_len];

            let mut reader: &[u8] = encoded;
            let raw = decode::decode_to_raw(&mut reader, decode::Config::default())?;

            align_to_4(&mut new_bin);
            let block_start = new_bin.len();
            new_bin.extend_from_slice(&raw.data);

            // Indices.
            let index_bv_index = buffer_views.len();
            buffer_views.push(json!({
                "buffer": target_buffer_index,
                "byteOffset": block_start + raw.indices_offset,
                "byteLength": raw.indices_byte_length,
                "target": 34963u32, // ELEMENT_ARRAY_BUFFER
            }));
            let index_component_type = raw
                .indices_component_type
                .to_gltf_component_type()
                .unwrap_or(5125);
            if let Some(idx) = old_index_accessor {
                if let Some(acc) = accessors.get_mut(idx).and_then(|v| v.as_object_mut()) {
                    acc.insert("bufferView".into(), json!(index_bv_index));
                    acc.insert("byteOffset".into(), json!(0));
                    acc.insert("componentType".into(), json!(index_component_type));
                    acc.insert("count".into(), json!(raw.index_count));
                    acc.insert("type".into(), json!("SCALAR"));
                }
            }

            // Map: gltf semantic name → draco unique_id (from extension).
            // Inverted to look up the gltf accessor for each decoded attribute.
            let attr_map = ext
                .get("attributes")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let mut by_unique_id: std::collections::HashMap<u32, String> =
                std::collections::HashMap::new();
            for (gltf_name, draco_id_v) in attr_map.into_iter() {
                if let Some(uid) = draco_id_v.as_u64() {
                    by_unique_id.insert(uid as u32, gltf_name);
                }
            }

            for attr in &raw.attributes {
                let semantic_owned: Option<String> = attr.gltf_semantic.map(|s| s.to_string());
                let gltf_name = match by_unique_id
                    .get(&(attr.unique_id as u32))
                    .cloned()
                    .or(semantic_owned)
                {
                    Some(n) => n,
                    None => continue,
                };
                let Some(&existing_acc_idx) = old_attr_accessors.get(&gltf_name) else {
                    continue;
                };
                let bv_index = buffer_views.len();
                buffer_views.push(json!({
                    "buffer": target_buffer_index,
                    "byteOffset": block_start + attr.offset,
                    "byteLength": attr.byte_length,
                    "target": 34962u32, // ARRAY_BUFFER
                }));
                let component_type = attr.component_type.to_gltf_component_type().unwrap_or(5126);
                let accessor_type = accessor_type_from_dim(attr.dim);
                if let Some(acc) = accessors.get_mut(existing_acc_idx).and_then(|v| v.as_object_mut()) {
                    acc.insert("bufferView".into(), json!(bv_index));
                    acc.insert("byteOffset".into(), json!(0));
                    acc.insert("componentType".into(), json!(component_type));
                    acc.insert("count".into(), json!(raw.vertex_count));
                    acc.insert("type".into(), json!(accessor_type));
                    if gltf_name == "POSITION" && attr.component_type == ComponentDataType::F32 {
                        let pos_bytes = &raw.data[attr.offset..attr.offset + attr.byte_length];
                        if let Some((mins, maxs)) = compute_position_min_max(pos_bytes, attr.dim as usize) {
                            acc.insert("min".into(), json!(mins));
                            acc.insert("max".into(), json!(maxs));
                        }
                    }
                }
            }

            if let Some(exts) = prim.get_mut("extensions").and_then(|v| v.as_object_mut()) {
                exts.remove(draco_extension::EXTENSION_NAME);
                if exts.is_empty() {
                    if let Some(prim_obj) = prim.as_object_mut() {
                        prim_obj.remove("extensions");
                    }
                }
            }

            prims_new.push(prim);
        }

        if let Some(obj) = mesh.as_object_mut() {
            obj.insert("primitives".into(), Value::Array(prims_new));
        }
        meshes_new.push(mesh);
    }

    json["meshes"] = Value::Array(meshes_new);
    json["bufferViews"] = Value::Array(buffer_views);
    json["accessors"] = Value::Array(accessors);

    if let Some(buffers) = json.get_mut("buffers").and_then(|v| v.as_array_mut()) {
        if let Some(buf0) = buffers.get_mut(target_buffer_index) {
            buf0["byteLength"] = json!(new_bin.len());
        }
    } else {
        json["buffers"] = json!([{ "byteLength": new_bin.len() }]);
    }

    strip_ext_from_array(&mut json, "extensionsRequired");
    strip_ext_from_array(&mut json, "extensionsUsed");

    let new_json_bytes = serde_json::to_vec(&json)?;
    let mut out = Vec::with_capacity(20 + new_json_bytes.len() + new_bin.len());
    glb::write_glb(&mut out, &new_json_bytes, &new_bin)?;
    Ok(out)
}

fn json_has_draco_primitive(json: &Value) -> bool {
    let Some(meshes) = json.get("meshes").and_then(|v| v.as_array()) else {
        return false;
    };
    meshes.iter().any(|mesh| {
        mesh.get("primitives")
            .and_then(|v| v.as_array())
            .map(|prims| {
                prims
                    .iter()
                    .any(|p| draco_extension::is_draco_compressed(p))
            })
            .unwrap_or(false)
    })
}

fn read_buffer_view_range(
    buffer_views: &[Value],
    idx: usize,
) -> Result<(usize, usize), Error> {
    let bv = buffer_views.get(idx).ok_or(Error::BufferViewOutOfRange(idx))?;
    let byte_offset = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let byte_length = bv.get("byteLength").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    Ok((byte_offset, byte_length))
}

fn strip_ext_from_array(json: &mut Value, key: &str) {
    let Some(arr) = json.get_mut(key).and_then(|v| v.as_array_mut()) else {
        return;
    };
    arr.retain(|v| v.as_str() != Some(draco_extension::EXTENSION_NAME));
    if arr.is_empty() {
        if let Some(obj) = json.as_object_mut() {
            obj.remove(key);
        }
    }
}

fn align_to_4(buf: &mut Vec<u8>) {
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
}

fn accessor_type_from_dim(dim: u8) -> &'static str {
    match dim {
        1 => "SCALAR",
        2 => "VEC2",
        3 => "VEC3",
        4 => "VEC4",
        _ => "SCALAR",
    }
}

/// (mins, maxs) per component over an f32 attribute buffer. Required by
/// glTF 2.0 spec for POSITION accessors.
fn compute_position_min_max(bytes: &[u8], dim: usize) -> Option<(Vec<f32>, Vec<f32>)> {
    if dim == 0 {
        return None;
    }
    let stride = dim * 4;
    if bytes.is_empty() || bytes.len() % stride != 0 {
        return None;
    }
    let mut mins = vec![f32::INFINITY; dim];
    let mut maxs = vec![f32::NEG_INFINITY; dim];
    for vertex in bytes.chunks_exact(stride) {
        for (i, comp) in vertex.chunks_exact(4).enumerate() {
            let v = f32::from_le_bytes([comp[0], comp[1], comp[2], comp[3]]);
            if v < mins[i] {
                mins[i] = v;
            }
            if v > maxs[i] {
                maxs[i] = v;
            }
        }
    }
    if mins.iter().any(|v| !v.is_finite()) || maxs.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some((mins, maxs))
}

/// Slice the bytes for a specific bufferView out of the binary buffer.
fn extract_buffer_view(json: &Value, binary_buffer: &[u8], view_idx: usize) -> Result<Vec<u8>, Error> {
    let view = json
        .get("bufferViews")
        .and_then(|v| v.as_array())
        .and_then(|v| v.get(view_idx))
        .ok_or(Error::MissingField("bufferViews[idx]", format!("bufferViews[{}]", view_idx)))?;
    let buffer = view.get("buffer").and_then(|b| b.as_u64()).unwrap_or(0) as usize;
    if buffer != 0 {
        return Err(Error::NonZeroBuffer(view_idx, buffer));
    }
    let offset = view.get("byteOffset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
    let length = view
        .get("byteLength")
        .and_then(|l| l.as_u64())
        .ok_or(Error::MissingField("byteLength", format!("bufferViews[{}]", view_idx)))?
        as usize;
    let end = offset.checked_add(length).ok_or(Error::BufferViewOutOfRange(view_idx))?;
    if end > binary_buffer.len() {
        return Err(Error::BufferViewOutOfRange(view_idx));
    }
    Ok(binary_buffer[offset..end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::gltf::transcoder::GltfTranscoder;
    use std::path::PathBuf;

    /// End-to-end test: take a vanilla glb, run our encoder/transcoder
    /// to add Draco compression, then run our decoder over the result.
    /// Validates that the encode → decode pair survives the full glTF
    /// extension wrapping, exercising `decode_glb` end-to-end.
    #[test]
    fn glb_roundtrip_via_transcoder() {
        let in_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/Duck/Duck.glb");
        // Skip if the bundled Duck fixture isn't present.
        if !in_path.exists() {
            eprintln!("[SKIP] {} missing", in_path.display());
            return;
        }
        let raw = std::fs::read(&in_path).expect("read input glb");

        let transcoder = GltfTranscoder::default();
        let (compressed, warnings) = transcoder.transcode_to_glb(&raw).expect("transcode");
        for w in &warnings {
            eprintln!("transcode warning: {}", w);
        }

        let decoded = decode_glb(&compressed).expect("decode");
        assert!(
            !decoded.is_empty(),
            "expected at least one Draco-compressed primitive"
        );
        for p in &decoded {
            assert!(
                p.mesh.get_faces().len() > 0,
                "primitive {}/{} has 0 faces",
                p.mesh_idx, p.primitive_idx
            );
            let has_pos = p
                .mesh
                .get_attributes()
                .iter()
                .any(|a| a.get_attribute_type() == crate::prelude::AttributeType::Position);
            assert!(has_pos, "primitive {}/{} missing positions", p.mesh_idx, p.primitive_idx);
        }
        eprintln!(
            "decoded {} Draco primitives from glb-roundtripped Duck",
            decoded.len()
        );
    }

    /// `splice_glb_remove_draco` should pass through GLBs without the
    /// extension verbatim.
    #[test]
    fn splice_passthrough_when_no_draco() {
        let json = br#"{"asset":{"version":"2.0"}}"#;
        let mut glb = Vec::new();
        glb::write_glb(&mut glb, json, b"\x01\x02\x03\x04").unwrap();
        let out = splice_glb_remove_draco(&glb).expect("splice");
        assert_eq!(out, glb);
    }

    /// End-to-end: vanilla glb → transcode (add Draco) →
    /// `splice_glb_remove_draco` (strip Draco) → result is parseable as
    /// vanilla glTF and the extension is gone.
    #[test]
    fn splice_glb_roundtrip_via_transcoder() {
        let in_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/Duck/Duck.glb");
        if !in_path.exists() {
            eprintln!("[SKIP] {} missing", in_path.display());
            return;
        }
        let raw = std::fs::read(&in_path).expect("read input glb");

        let transcoder = GltfTranscoder::default();
        let (compressed, _warnings) = transcoder.transcode_to_glb(&raw).expect("transcode");

        let stripped = splice_glb_remove_draco(&compressed).expect("splice");
        let parsed = glb::parse_glb(&stripped).expect("parse spliced glb");
        let json: Value = serde_json::from_slice(&parsed.json).expect("json");

        // Extension references gone.
        assert!(
            !json
                .get("extensionsRequired")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|s| s.as_str() == Some(draco_extension::EXTENSION_NAME)))
                .unwrap_or(false),
            "extensionsRequired still references {}",
            draco_extension::EXTENSION_NAME
        );
        assert!(
            !json
                .get("extensionsUsed")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|s| s.as_str() == Some(draco_extension::EXTENSION_NAME)))
                .unwrap_or(false),
            "extensionsUsed still references {}",
            draco_extension::EXTENSION_NAME
        );
        // No primitive carries the extension anymore.
        if let Some(meshes) = json.get("meshes").and_then(|v| v.as_array()) {
            for m in meshes {
                if let Some(prims) = m.get("primitives").and_then(|v| v.as_array()) {
                    for p in prims {
                        assert!(
                            !draco_extension::is_draco_compressed(p),
                            "primitive still carries {}",
                            draco_extension::EXTENSION_NAME
                        );
                    }
                }
            }
        }
    }

    /// Regression test: an earlier `splice_glb_remove_draco` emitted
    /// attribute accessors whose `count` was the unified `vertex_count`
    /// but whose backing bufferView byteLength came from the per-attribute
    /// Draco vertex count (smaller when the mesh has NORMAL/TEXCOORD
    /// seams). bevy_gltf rejected every such accessor as `MalformedData`.
    ///
    /// Invariants this asserts for every accessor patched by the splice:
    /// 1. `bufferView.byteLength == accessor.count * dim_bytes(accessor)`
    /// 2. Every index in the indices bufferView is `< POSITION.count`
    ///    (which must equal every other attribute's `count`).
    /// 3. All attribute accessors on a Draco-stripped primitive share
    ///    one `count`.
    #[test]
    fn splice_emits_consistent_accessor_counts() {
        let in_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/Duck/Duck.glb");
        if !in_path.exists() {
            eprintln!("[SKIP] {} missing", in_path.display());
            return;
        }
        let raw = std::fs::read(&in_path).expect("read input glb");

        let transcoder = GltfTranscoder::default();
        let (compressed, _warnings) = transcoder.transcode_to_glb(&raw).expect("transcode");

        let stripped = splice_glb_remove_draco(&compressed).expect("splice");
        let parsed = glb::parse_glb(&stripped).expect("parse spliced glb");
        let json: Value = serde_json::from_slice(&parsed.json).expect("json");

        let accessors = json
            .get("accessors")
            .and_then(|v| v.as_array())
            .expect("accessors");
        let buffer_views = json
            .get("bufferViews")
            .and_then(|v| v.as_array())
            .expect("bufferViews");

        let bv_len = |idx: usize| -> usize {
            buffer_views[idx]
                .get("byteLength")
                .and_then(|v| v.as_u64())
                .unwrap() as usize
        };

        let component_size = |code: u64| -> usize {
            match code {
                5120 | 5121 => 1,
                5122 | 5123 => 2,
                5125 | 5126 => 4,
                _ => panic!("unexpected componentType {}", code),
            }
        };
        let type_dim = |t: &str| -> usize {
            match t {
                "SCALAR" => 1,
                "VEC2" => 2,
                "VEC3" => 3,
                "VEC4" => 4,
                _ => panic!("unexpected type {}", t),
            }
        };

        let meshes = json.get("meshes").and_then(|v| v.as_array()).expect("meshes");
        let mut checked_primitives = 0usize;
        for mesh in meshes {
            let Some(prims) = mesh.get("primitives").and_then(|v| v.as_array()) else {
                continue;
            };
            for prim in prims {
                let Some(attr_map) = prim.get("attributes").and_then(|v| v.as_object()) else {
                    continue;
                };
                if attr_map.is_empty() {
                    continue;
                }
                checked_primitives += 1;

                let attr_accessors: Vec<usize> = attr_map
                    .values()
                    .filter_map(|v| v.as_u64().map(|u| u as usize))
                    .collect();
                let counts: Vec<u64> = attr_accessors
                    .iter()
                    .map(|&i| accessors[i].get("count").and_then(|v| v.as_u64()).unwrap())
                    .collect();
                let unified = counts[0];
                for &c in &counts {
                    assert_eq!(
                        c, unified,
                        "all attribute accessors on a primitive must share one count, got {:?}",
                        counts
                    );
                }

                // Invariant 1: byteLength == count * elem_size for every accessor.
                for &acc_idx in &attr_accessors {
                    let acc = &accessors[acc_idx];
                    let count = acc.get("count").and_then(|v| v.as_u64()).unwrap() as usize;
                    let ct = acc.get("componentType").and_then(|v| v.as_u64()).unwrap();
                    let ty = acc.get("type").and_then(|v| v.as_str()).unwrap();
                    let bv_idx = acc.get("bufferView").and_then(|v| v.as_u64()).unwrap() as usize;
                    let expected = count * component_size(ct) * type_dim(ty);
                    let actual = bv_len(bv_idx);
                    assert_eq!(
                        actual, expected,
                        "accessor {} bufferView byteLength {} != count {} * elem_size {}",
                        acc_idx, actual, count, expected / count.max(1)
                    );
                }

                // Invariant 2: every index < unified count.
                let Some(idx_acc_n) = prim.get("indices").and_then(|v| v.as_u64()) else {
                    continue;
                };
                let idx_acc = &accessors[idx_acc_n as usize];
                let idx_count = idx_acc.get("count").and_then(|v| v.as_u64()).unwrap() as usize;
                let idx_ct = idx_acc.get("componentType").and_then(|v| v.as_u64()).unwrap();
                let idx_bv = idx_acc.get("bufferView").and_then(|v| v.as_u64()).unwrap() as usize;
                let bv = &buffer_views[idx_bv];
                let bv_off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap() as usize;
                let bv_len_total = bv.get("byteLength").and_then(|v| v.as_u64()).unwrap() as usize;
                let bytes = &parsed.buffer[bv_off..bv_off + bv_len_total];
                let unified_usize = unified as usize;
                if idx_ct == 5123 {
                    assert_eq!(bytes.len(), idx_count * 2);
                    for chunk in bytes.chunks_exact(2) {
                        let i = u16::from_le_bytes([chunk[0], chunk[1]]) as usize;
                        assert!(i < unified_usize, "index {} >= vertex_count {}", i, unified_usize);
                    }
                } else if idx_ct == 5125 {
                    assert_eq!(bytes.len(), idx_count * 4);
                    for chunk in bytes.chunks_exact(4) {
                        let i = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as usize;
                        assert!(i < unified_usize, "index {} >= vertex_count {}", i, unified_usize);
                    }
                } else {
                    panic!("unexpected index componentType {}", idx_ct);
                }
            }
        }

        assert!(checked_primitives > 0, "no primitives with attributes were checked");
    }
}
