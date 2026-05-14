//! Side-by-side comparison of our `decode_to_raw` vs Google's
//! reference `draco_decoder` for the same Draco bytes (extracted
//! from a b3dm). Run as:
//!
//!     cargo run --example diff_against_google -- path/to/file.b3dm
//!
//! Prints per-attribute value divergence so we can localize the bug:
//!   - position bin agreement (unique + multiset)
//!   - position-triangle multiset agreement
//!   - per-vertex (matched by position) normal & UV L_inf
//!   - per-corner tuple multiset agreement
//!
//! Native-only — uses `draco_decoder = "0.0.26"` as a build dep.

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: diff_against_google <input.b3dm>");
    let bytes = std::fs::read(&path).expect("read input");

    let glb = if &bytes[0..4] == b"b3dm" {
        strip_b3dm_header(&bytes).expect("strip b3dm")
    } else {
        bytes.clone()
    };

    // Splice via draco-oxide and inspect the result's accessors.
    if &glb[0..4] == b"glTF" {
        eprintln!("\n=== unique_id alignment ===");
        inspect_unique_id_alignment(&glb);

        eprintln!("\n=== splice diagnostic ===");
        match draco_oxide::prelude::splice_glb_remove_draco(&glb) {
            Ok(spliced) => inspect_spliced(&spliced),
            Err(e) => eprintln!("splice failed: {}", e),
        }
    }

    let drcs = if &bytes[0..4] == b"b3dm" {
        extract_draco_primitives(&glb).expect("extract Draco primitives")
    } else if &bytes[0..4] == b"glTF" {
        extract_draco_primitives(&bytes).expect("extract Draco primitives")
    } else {
        vec![bytes]
    };
    eprintln!(
        "\n{}: {} Draco primitive(s)",
        path,
        drcs.len()
    );
    for (i, drc) in drcs.iter().enumerate() {
        eprintln!("\n=== primitive {} ({} bytes Draco) ===", i, drc.len());
        diff_one(drc);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn inspect_unique_id_alignment(glb: &[u8]) {
    use draco_oxide::prelude::{decode, decode_to_raw, ConfigType};
    use serde_json::Value;
    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    let json: Value = serde_json::from_slice(&glb[20..20 + json_len]).expect("json");
    let bin_start = 20 + json_len + 8;
    let bin_len = u32::from_le_bytes([glb[20 + json_len], glb[20 + json_len + 1], glb[20 + json_len + 2], glb[20 + json_len + 3]]) as usize;
    let bin = &glb[bin_start..bin_start + bin_len];
    let bvs = json.get("bufferViews").and_then(|v| v.as_array()).unwrap();

    for (mi, mesh) in json.get("meshes").and_then(|v| v.as_array()).unwrap_or(&Vec::new()).iter().enumerate() {
        for (pi, prim) in mesh.get("primitives").and_then(|v| v.as_array()).unwrap_or(&Vec::new()).iter().enumerate() {
            let Some(ext) = prim.get("extensions").and_then(|e| e.get("KHR_draco_mesh_compression")) else { continue; };
            let Some(attrs) = ext.get("attributes").and_then(|v| v.as_object()) else { continue; };
            eprintln!("  mesh[{}].primitive[{}] GLB extension's attributes map (gltf_name -> draco_unique_id):", mi, pi);
            for (k, v) in attrs {
                eprintln!("    {} -> {}", k, v);
            }
            let bv_idx = ext.get("bufferView").and_then(|v| v.as_u64()).unwrap() as usize;
            let bv = bvs.get(bv_idx).unwrap();
            let off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let len = bv.get("byteLength").and_then(|v| v.as_u64()).unwrap() as usize;
            let mut reader = bin[off..off + len].to_vec().into_iter();
            let raw = decode_to_raw(&mut reader, decode::Config::default()).unwrap();
            eprintln!("  draco-oxide decoded attributes (unique_id, gltf_semantic, dim, type):");
            for a in &raw.attributes {
                eprintln!("    unique_id={} semantic={:?} dim={} type={:?}", a.unique_id, a.gltf_semantic, a.dim, a.component_type);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn inspect_spliced(glb: &[u8]) {
    use serde_json::Value;
    if &glb[0..4] != b"glTF" {
        eprintln!("  not a GLB");
        return;
    }
    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    let json: Value = match serde_json::from_slice(&glb[20..20 + json_len]) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("  JSON parse failed: {}", e);
            return;
        }
    };
    eprintln!("  GLB size: {} bytes (JSON: {} bytes)", glb.len(), json_len);
    let buffers = json.get("buffers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    eprintln!("  buffers: {} entries", buffers.len());
    for (i, b) in buffers.iter().enumerate() {
        eprintln!("    [{}] byteLength={:?} uri={:?}", i,
            b.get("byteLength").and_then(|v| v.as_u64()),
            b.get("uri").and_then(|v| v.as_str()));
    }
    let bvs = json.get("bufferViews").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let accessors = json.get("accessors").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    eprintln!("  bufferViews: {}, accessors: {}", bvs.len(), accessors.len());

    let meshes = json.get("meshes").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for (mi, mesh) in meshes.iter().enumerate() {
        let prims = mesh.get("primitives").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for (pi, prim) in prims.iter().enumerate() {
            eprintln!("\n  mesh[{}].primitive[{}]:", mi, pi);
            if let Some(ext) = prim.get("extensions").and_then(|e| e.get("KHR_draco_mesh_compression")) {
                eprintln!("    STILL HAS DRACO EXTENSION: {:?}", ext);
            }
            // indices
            if let Some(idx) = prim.get("indices").and_then(|v| v.as_u64()) {
                inspect_accessor(&accessors, &bvs, idx as usize, "indices");
            }
            // attributes
            if let Some(attrs) = prim.get("attributes").and_then(|v| v.as_object()) {
                for (name, v) in attrs {
                    let idx = v.as_u64().unwrap_or(0) as usize;
                    inspect_accessor(&accessors, &bvs, idx, name);
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn inspect_accessor(accessors: &[serde_json::Value], bvs: &[serde_json::Value], idx: usize, label: &str) {
    let acc = match accessors.get(idx) {
        Some(a) => a,
        None => {
            eprintln!("    {}: accessor[{}] MISSING", label, idx);
            return;
        }
    };
    let bv_idx = acc.get("bufferView").and_then(|v| v.as_u64());
    let byte_offset = acc.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0);
    let ct = acc.get("componentType").and_then(|v| v.as_u64()).unwrap_or(0);
    let count = acc.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    let ty = acc.get("type").and_then(|v| v.as_str()).unwrap_or("?");
    let min = acc.get("min").and_then(|v| v.as_array());
    let max = acc.get("max").and_then(|v| v.as_array());
    let ct_name = match ct {
        5120 => "I8",
        5121 => "U8",
        5122 => "I16",
        5123 => "U16",
        5125 => "U32",
        5126 => "F32",
        _ => "?",
    };
    let elem_bytes = match ct {
        5120 | 5121 => 1usize,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        _ => 0,
    };
    let dim = match ty {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => 0,
    };
    let expected_bytes = count as usize * elem_bytes * dim;
    let bv_info = bv_idx.and_then(|i| bvs.get(i as usize)).map(|bv| {
        let buf = bv.get("buffer").and_then(|v| v.as_u64()).unwrap_or(0);
        let off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0);
        let len = bv.get("byteLength").and_then(|v| v.as_u64()).unwrap_or(0);
        let stride = bv.get("byteStride").and_then(|v| v.as_u64());
        format!(
            "buf={} bvOff={} bvLen={} stride={:?}",
            buf, off, len, stride,
        )
    }).unwrap_or_else(|| "?".to_string());
    let mismatch = if bv_idx.is_some() {
        let actual_bv_len = bvs
            .get(bv_idx.unwrap() as usize)
            .and_then(|bv| bv.get("byteLength").and_then(|v| v.as_u64()))
            .unwrap_or(0) as usize;
        if actual_bv_len < expected_bytes {
            format!(" *** BV TOO SHORT (need {} got {}) ***", expected_bytes, actual_bv_len)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    eprintln!(
        "    {:16} accessor[{}] {}×{} ({}) count={} byteOff={} bv=[{}]{}{}{}",
        label,
        idx,
        ct_name,
        dim,
        ty,
        count,
        byte_offset,
        bv_info,
        if min.is_some() { format!(" min={:?}", min.unwrap()) } else { String::new() },
        if max.is_some() { format!(" max={:?}", max.unwrap()) } else { String::new() },
        mismatch,
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn diff_one(drc: &[u8]) {
    use draco_decoder::AttributeDataType as GAttrTy;
    use draco_oxide::prelude::{decode, decode_to_raw, ComponentDataType, ConfigType};
    use std::collections::HashMap;

    let google = match draco_decoder::decode_mesh_with_config_sync(drc) {
        Some(g) => g,
        None => {
            eprintln!("google decode failed");
            return;
        }
    };
    let mut reader = drc.to_vec().into_iter();
    let our = match decode_to_raw(&mut reader, decode::Config::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("our decode_to_raw failed: {}", e);
            return;
        }
    };

    eprintln!(
        "  vertex_count: ours={}, google={} ({}match)",
        our.vertex_count,
        google.config.vertex_count(),
        if our.vertex_count == google.config.vertex_count() { "" } else { "MIS" },
    );
    eprintln!(
        "  index_count:  ours={}, google={} ({}match)",
        our.index_count,
        google.config.index_count(),
        if our.index_count == google.config.index_count() { "" } else { "MIS" },
    );

    // Pull positions / normals / UVs from both.
    let read_f32 = |bytes: &[u8], dim: usize, count: usize| -> Vec<Vec<f32>> {
        let stride = dim * 4;
        if bytes.len() != count * stride {
            eprintln!(
                "  WARN: byte count {} != count {} * stride {}",
                bytes.len(), count, stride
            );
        }
        bytes
            .chunks_exact(stride)
            .map(|row| row.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
            .collect()
    };
    let read_indices = |bytes: &[u8], byte_count: usize, vertex_count: u32| -> Vec<usize> {
        if vertex_count <= u16::MAX as u32 {
            bytes[..byte_count]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]) as usize)
                .collect()
        } else {
            bytes[..byte_count]
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize)
                .collect()
        }
    };

    let our_pos_attr = our.attributes.iter().find(|a| a.gltf_semantic == Some("POSITION"));
    let our_norm_attr = our.attributes.iter().find(|a| a.gltf_semantic == Some("NORMAL"));
    let our_uv_attr = our.attributes.iter().find(|a| a.gltf_semantic == Some("TEXCOORD_0"));

    let g_attrs = google.config.attributes();
    let mut g_pos = None;
    let mut g_norm = None;
    for a in &g_attrs {
        if a.dim() == 3 && a.data_type() == GAttrTy::Float32 {
            let off = a.offset() as usize;
            let v = [
                f32::from_le_bytes([google.data[off], google.data[off+1], google.data[off+2], google.data[off+3]]),
                f32::from_le_bytes([google.data[off+4], google.data[off+5], google.data[off+6], google.data[off+7]]),
                f32::from_le_bytes([google.data[off+8], google.data[off+9], google.data[off+10], google.data[off+11]]),
            ];
            let mag = (v[0]*v[0] + v[1]*v[1] + v[2]*v[2]).sqrt();
            if mag > 1.5 { g_pos = Some(*a); } else { g_norm = Some(*a); }
        }
    }
    let g_uv = g_attrs.iter().find(|a| a.dim() == 2 && a.data_type() == GAttrTy::Float32).copied();

    let our_positions = our_pos_attr.map(|a| read_f32(
        &our.data[a.offset..a.offset + a.byte_length],
        a.dim as usize,
        our.vertex_count as usize,
    )).expect("ours has POSITION");
    let google_positions = g_pos.map(|a| read_f32(
        &google.data[a.offset() as usize..a.offset() as usize + a.lenght() as usize],
        a.dim() as usize,
        google.config.vertex_count() as usize,
    )).expect("google has POSITION");

    let our_indices = read_indices(&our.data[..our.indices_byte_length], our.indices_byte_length, our.vertex_count);
    let google_indices = read_indices(&google.data[..google.config.index_length() as usize], google.config.index_length() as usize, google.config.vertex_count());

    let pos_q = 0.01_f32;
    let bin = |v: f32| (v / pos_q).round() as i32;

    // 1. Position bin agreement.
    let our_pos_bins: Vec<[i32;3]> = our_positions.iter().map(|p| [bin(p[0]), bin(p[1]), bin(p[2])]).collect();
    let google_pos_bins: Vec<[i32;3]> = google_positions.iter().map(|p| [bin(p[0]), bin(p[1]), bin(p[2])]).collect();
    let our_pos_unique: std::collections::HashSet<[i32;3]> = our_pos_bins.iter().copied().collect();
    let google_pos_unique: std::collections::HashSet<[i32;3]> = google_pos_bins.iter().copied().collect();
    let common = our_pos_unique.intersection(&google_pos_unique).count();
    eprintln!("  position bins (1cm grid): unique ours={} google={} common={}",
        our_pos_unique.len(), google_pos_unique.len(), common);

    // 2. Position-triangle multiset.
    let mut our_pos_tri: HashMap<[[i32;3];3], usize> = HashMap::new();
    let mut google_pos_tri: HashMap<[[i32;3];3], usize> = HashMap::new();
    for chunk in our_indices.chunks_exact(3) {
        let mut t = [our_pos_bins[chunk[0]], our_pos_bins[chunk[1]], our_pos_bins[chunk[2]]];
        t.sort();
        *our_pos_tri.entry(t).or_insert(0) += 1;
    }
    for chunk in google_indices.chunks_exact(3) {
        let mut t = [google_pos_bins[chunk[0]], google_pos_bins[chunk[1]], google_pos_bins[chunk[2]]];
        t.sort();
        *google_pos_tri.entry(t).or_insert(0) += 1;
    }
    let only_ours = our_pos_tri.iter().filter(|(k,_)| !google_pos_tri.contains_key(*k)).count();
    let only_google = google_pos_tri.iter().filter(|(k,_)| !our_pos_tri.contains_key(*k)).count();
    let pos_tri_match = our_pos_tri == google_pos_tri;
    eprintln!("  position triangles: ours={} google={} match={} (only_ours={} only_google={})",
        our_pos_tri.len(), google_pos_tri.len(), pos_tri_match, only_ours, only_google);

    // Dump the wrong-only-on-ours faces and the wrong-only-on-google
    // faces. If their position bins overlap, this is likely a "swapped
    // diagonal" pattern (quad split the wrong way) vs a wholesale
    // mis-routing.
    if !pos_tri_match {
        let mut ours_only: Vec<&[[i32;3]; 3]> = our_pos_tri.keys().filter(|k| !google_pos_tri.contains_key(*k)).collect();
        let mut google_only: Vec<&[[i32;3]; 3]> = google_pos_tri.keys().filter(|k| !our_pos_tri.contains_key(*k)).collect();
        ours_only.sort();
        google_only.sort();
        eprintln!("\n  WRONG faces (in ours, not google):");
        for f in ours_only.iter().take(20) {
            eprintln!("    {:?}", f);
        }
        eprintln!("  WRONG faces (in google, not ours):");
        for f in google_only.iter().take(20) {
            eprintln!("    {:?}", f);
        }
        // For each ours-only face, find google-only faces that share at
        // least 2 of the 3 vertex bins. If many do, the bug is "wrong
        // diagonal in a quad" — the same 4 vertices form 2 triangles
        // both ways.
        let google_set: std::collections::HashSet<[[i32;3]; 3]> = google_only.iter().map(|f| **f).collect();
        let mut shared_diag = 0usize;
        for our_f in &ours_only {
            let our_set: std::collections::HashSet<[i32;3]> = our_f.iter().copied().collect();
            for google_f in &google_set {
                let g_set: std::collections::HashSet<[i32;3]> = google_f.iter().copied().collect();
                if our_set.intersection(&g_set).count() == 2 {
                    shared_diag += 1;
                    break;
                }
            }
        }
        eprintln!(
            "  of {} ours-only faces, {} share >=2 vertices with some google-only face",
            ours_only.len(), shared_diag,
        );
    }

    // 3. Match each our-vertex to nearest google-vertex by position bin.
    // Then compare normal & UV values at matched vertices.
    let mut google_by_bin: HashMap<[i32;3], Vec<usize>> = HashMap::new();
    for (i, b) in google_pos_bins.iter().enumerate() {
        google_by_bin.entry(*b).or_default().push(i);
    }

    let our_normals = our_norm_attr.map(|a| read_f32(
        &our.data[a.offset..a.offset + a.byte_length],
        a.dim as usize,
        our.vertex_count as usize,
    ));
    let our_uvs = our_uv_attr.map(|a| read_f32(
        &our.data[a.offset..a.offset + a.byte_length],
        a.dim as usize,
        our.vertex_count as usize,
    ));
    let google_normals = g_norm.map(|a| read_f32(
        &google.data[a.offset() as usize..a.offset() as usize + a.lenght() as usize],
        a.dim() as usize,
        google.config.vertex_count() as usize,
    ));
    let google_uvs = g_uv.map(|a| read_f32(
        &google.data[a.offset() as usize..a.offset() as usize + a.lenght() as usize],
        a.dim() as usize,
        google.config.vertex_count() as usize,
    ));

    let _ = google_by_bin;
    // Per-vertex (pos, norm, uv) tuple multiset. Each unique tuple is
    // a unique deduped vertex on Google's side; ours should produce
    // the same set of tuples. Mismatches here are conclusive evidence
    // of decoder divergence — vertex-numbering permutation between
    // the two decoders is OK, but each tuple must exist on both sides
    // with the same multiplicity.
    let norm_q = 0.01_f32;
    let uv_q = 0.001_f32;
    let nb = |v: f32| (v / norm_q).round() as i32;
    let ub = |v: f32| (v / uv_q).round() as i32;

    type TupleKey = ([i32;3], Option<[i32;3]>, Option<[i32;2]>);
    let mut our_tuples: HashMap<TupleKey, usize> = HashMap::new();
    let mut google_tuples: HashMap<TupleKey, usize> = HashMap::new();
    for vi in 0..our_positions.len() {
        let p = &our_positions[vi];
        let pk = [bin(p[0]), bin(p[1]), bin(p[2])];
        let nk = our_normals.as_ref().map(|n| {
            let v = &n[vi]; [nb(v[0]), nb(v[1]), nb(v[2])]
        });
        let uk = our_uvs.as_ref().map(|u| {
            let v = &u[vi]; [ub(v[0]), ub(v[1])]
        });
        *our_tuples.entry((pk, nk, uk)).or_insert(0) += 1;
    }
    for vi in 0..google_positions.len() {
        let p = &google_positions[vi];
        let pk = [bin(p[0]), bin(p[1]), bin(p[2])];
        let nk = google_normals.as_ref().map(|n| {
            let v = &n[vi]; [nb(v[0]), nb(v[1]), nb(v[2])]
        });
        let uk = google_uvs.as_ref().map(|u| {
            let v = &u[vi]; [ub(v[0]), ub(v[1])]
        });
        *google_tuples.entry((pk, nk, uk)).or_insert(0) += 1;
    }
    let only_ours_t = our_tuples.iter().filter(|(k,_)| !google_tuples.contains_key(*k)).count();
    let only_google_t = google_tuples.iter().filter(|(k,_)| !our_tuples.contains_key(*k)).count();
    eprintln!(
        "  per-vertex (pos,norm,uv) tuples: ours={} google={} match={} (only_ours={} only_google={})",
        our_tuples.len(),
        google_tuples.len(),
        our_tuples == google_tuples,
        only_ours_t,
        only_google_t,
    );

    // For tuples that differ, decompose: how many disagree on
    // position alone? on normal alone? on UV alone?
    let mut our_pn: HashMap<([i32;3], Option<[i32;3]>), usize> = HashMap::new();
    let mut google_pn: HashMap<([i32;3], Option<[i32;3]>), usize> = HashMap::new();
    for ((p, n, _), c) in &our_tuples { *our_pn.entry((*p, *n)).or_insert(0) += c; }
    for ((p, n, _), c) in &google_tuples { *google_pn.entry((*p, *n)).or_insert(0) += c; }
    let pn_only_ours = our_pn.iter().filter(|(k,_)| !google_pn.contains_key(*k)).count();
    let pn_only_google = google_pn.iter().filter(|(k,_)| !our_pn.contains_key(*k)).count();
    eprintln!(
        "  per-vertex (pos,norm) tuples:    ours={} google={} (only_ours={} only_google={})",
        our_pn.len(), google_pn.len(), pn_only_ours, pn_only_google,
    );

    let mut our_pu: HashMap<([i32;3], Option<[i32;2]>), usize> = HashMap::new();
    let mut google_pu: HashMap<([i32;3], Option<[i32;2]>), usize> = HashMap::new();
    for ((p, _, u), c) in &our_tuples { *our_pu.entry((*p, *u)).or_insert(0) += c; }
    for ((p, _, u), c) in &google_tuples { *google_pu.entry((*p, *u)).or_insert(0) += c; }
    let pu_only_ours = our_pu.iter().filter(|(k,_)| !google_pu.contains_key(*k)).count();
    let pu_only_google = google_pu.iter().filter(|(k,_)| !our_pu.contains_key(*k)).count();
    eprintln!(
        "  per-vertex (pos,uv)   tuples:    ours={} google={} (only_ours={} only_google={})",
        our_pu.len(), google_pu.len(), pu_only_ours, pu_only_google,
    );

    // Diagnostic: list a few of our (pos, norm) tuples that don't
    // exist in google's output, plus the google normal at the same
    // position, to spot the divergence pattern.
    if let (Some(on), Some(gn)) = (our_normals.as_ref(), google_normals.as_ref()) {
        // Build google normal-set per position bin.
        let mut google_norms_at: HashMap<[i32;3], Vec<[i32;3]>> = HashMap::new();
        for vi in 0..google_positions.len() {
            let p = &google_positions[vi];
            let pk = [bin(p[0]), bin(p[1]), bin(p[2])];
            let v = &gn[vi];
            google_norms_at.entry(pk).or_default().push([nb(v[0]), nb(v[1]), nb(v[2])]);
        }
        let mut shown = 0usize;
        eprintln!("\n  Sample of OUR vertices whose (pos,norm) tuple is not in Google's output:");
        for vi in 0..our_positions.len() {
            if shown >= 8 { break; }
            let p = &our_positions[vi];
            let pk = [bin(p[0]), bin(p[1]), bin(p[2])];
            let v = &on[vi];
            let nk = [nb(v[0]), nb(v[1]), nb(v[2])];
            let key = (pk, Some(nk), None::<[i32;2]>);
            // we already keyed by full triple; redo without UV
            let google_pn_has = google_pn.contains_key(&(pk, Some(nk)));
            if google_pn_has { continue; }
            let google_norms = google_norms_at.get(&pk).cloned().unwrap_or_default();
            eprintln!(
                "    our[{}]: pos={:?} our_norm_bin={:?}, google norms at this pos: {:?}",
                vi,
                p,
                nk,
                google_norms,
            );
            let _ = key;
            shown += 1;
        }
    }

    // 4. Sample dump: first vertex from each side.
    eprintln!("\n  sample our[0]:    pos={:?}", our_positions[0]);
    if let Some(on) = &our_normals { eprintln!("                    norm={:?}", on[0]); }
    if let Some(ou) = &our_uvs { eprintln!("                    uv={:?}", ou[0]); }
    eprintln!("  sample google[0]: pos={:?}", google_positions[0]);
    if let Some(gn) = &google_normals { eprintln!("                    norm={:?}", gn[0]); }
    if let Some(gu) = &google_uvs { eprintln!("                    uv={:?}", gu[0]); }

    let _ = ComponentDataType::F32;
}

#[cfg(not(target_arch = "wasm32"))]
fn strip_b3dm_header(b3dm: &[u8]) -> Result<Vec<u8>, String> {
    if b3dm.len() < 28 { return Err("too short".into()); }
    let r = |off: usize| u32::from_le_bytes([b3dm[off], b3dm[off+1], b3dm[off+2], b3dm[off+3]]) as usize;
    let glb_start = 28 + r(12) + r(16) + r(20) + r(24);
    if glb_start > b3dm.len() { return Err("header overflow".into()); }
    Ok(b3dm[glb_start..].to_vec())
}

#[cfg(not(target_arch = "wasm32"))]
fn extract_draco_primitives(glb: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    use serde_json::Value;
    if &glb[0..4] != b"glTF" { return Err("not GLB".into()); }
    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    let json_bytes = &glb[20..20 + json_len];
    let bin_chunk_start = 20 + json_len;
    let bin_len = u32::from_le_bytes([glb[bin_chunk_start], glb[bin_chunk_start+1], glb[bin_chunk_start+2], glb[bin_chunk_start+3]]) as usize;
    let bin = &glb[bin_chunk_start + 8 .. bin_chunk_start + 8 + bin_len];
    let json: Value = serde_json::from_slice(json_bytes).map_err(|e| e.to_string())?;
    let buffer_views = json.get("bufferViews").and_then(|v| v.as_array()).ok_or("no bufferViews")?;
    let mut out = Vec::new();
    if let Some(meshes) = json.get("meshes").and_then(|v| v.as_array()) {
        for mesh in meshes {
            if let Some(prims) = mesh.get("primitives").and_then(|v| v.as_array()) {
                for prim in prims {
                    if let Some(bv_idx) = prim.get("extensions")
                        .and_then(|e| e.get("KHR_draco_mesh_compression"))
                        .and_then(|d| d.get("bufferView"))
                        .and_then(|v| v.as_u64()) {
                        let bv = buffer_views.get(bv_idx as usize).ok_or("bv oor")?;
                        let off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let len = bv.get("byteLength").and_then(|v| v.as_u64()).ok_or("no len")? as usize;
                        out.push(bin[off..off+len].to_vec());
                    }
                }
            }
        }
    }
    Ok(out)
}
