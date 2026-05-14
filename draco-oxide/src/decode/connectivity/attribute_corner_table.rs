//! Decoder-side per-attribute corner table.
//!
//! Mirrors `core/corner_table/attribute_corner_table.rs` but operates on
//! the decoded universal `DecoderCornerTable` plus a stream of
//! per-attribute seam bits. Used to give attribute-decode loops the
//! right `vertex_idx`/`opposite`/`left_most_corner` queries when
//! attributes (typically UVs and sometimes normals) split a vertex
//! across a seam edge.

use crate::core::corner_table::GenericCornerTable;
use crate::core::shared::{CornerIdx, FaceIdx, PointIdx, VertexIdx};
use crate::decode::connectivity::corner_table::{DecoderCornerTable, NO_CORNER};

/// A seam-aware corner table derived from the universal corner table
/// + per-attribute seam bits decoded from the connectivity bitstream.
///
/// `corner_to_vertex` is keyed by corner idx and yields *attribute*
/// vertex IDs (not universal). Because attribute seams split a single
/// universal vertex into multiple attribute vertices, `num_vertices()`
/// can be larger than the underlying `DecoderCornerTable::num_vertices()`.
pub(crate) struct DecoderAttributeCornerTable {
    pub(crate) corner_to_vertex: Vec<usize>,
    pub(crate) is_edge_on_seam: Vec<bool>,
    #[allow(dead_code)]
    pub(crate) is_vertex_on_seam: Vec<bool>,
    pub(crate) left_most_corners: Vec<usize>,
    pub(crate) num_vertices: usize,
    /// Copy of the universal CT's opposite[] so `opposite()` can answer
    /// without needing a borrow back to the universal CT. Seam edges
    /// are returned as None; non-seam edges return the universal
    /// opposite.
    pub(crate) opposite_universal: Vec<usize>,
}

impl DecoderAttributeCornerTable {
    /// Like `build_with_offsets`, but defaults all primary offsets to 0
    /// (matching the encoder's old behavior on positions-only meshes
    /// where the first decoder corner == encoder's primary corner).
    #[allow(dead_code)]
    pub(crate) fn build(ct: &DecoderCornerTable, seam_bits: &[bool]) -> Self {
        let num_faces = ct.num_faces();
        let zero_offsets = vec![0u8; num_faces];
        Self::build_with_offsets(ct, seam_bits, &zero_offsets)
    }

    /// Build from a universal corner table + decoded seam bits +
    /// per-face primary corner offsets.
    ///
    /// `seam_bits[i]` is the bit emitted for the i-th corner-pair in the
    /// encoder's reversed iteration order (= decoder face order). For
    /// each non-boundary corner whose opposite face hasn't been visited
    /// yet, a single bit is consumed; that bit becomes
    /// `is_edge_on_seam[corner]` (and is mirrored to the opposite
    /// corner). Boundary corners are always seams.
    ///
    /// `primary_offsets[face]` ∈ {0, 1, 2} reproduces the encoder's
    /// `[c, next(c), prev(c)]` iteration order for that face — without
    /// this, R/L symbols (where encoder's c isn't decoder's `base`)
    /// would associate bits to the wrong corners.
    pub(crate) fn build_with_offsets(
        ct: &DecoderCornerTable,
        seam_bits: &[bool],
        primary_offsets: &[u8],
    ) -> Self {
        let num_corners = ct.num_corners();
        let num_faces = ct.num_faces();
        let num_universal_vertices = {
            let mut max = 0usize;
            for c in 0..num_corners {
                let v = usize::from(ct.vertex_idx(CornerIdx::from(c)));
                if v > max {
                    max = v;
                }
            }
            max + 1
        };

        let mut is_edge_on_seam = vec![false; num_corners];
        let mut visited_faces = vec![false; num_faces];
        let mut bit_idx = 0usize;
        debug_assert_eq!(primary_offsets.len(), num_faces);
        // Match encoder iteration order: faces in REVERSE of decoder
        // face order. Skip start-face entries (sentinel u8::MAX) since
        // they were never in encoder's processed_connectivity_corners.
        // Boundary edges of skipped start-faces still need to be
        // marked as seams below.
        for f in (0..num_faces).rev() {
            let off_raw = primary_offsets[f];
            if off_raw == u8::MAX {
                continue;
            }
            let base = 3 * f;
            visited_faces[f] = true;
            let off = off_raw as usize;
            for k in 0..3 {
                let corner_offset = (off + k) % 3;
                let c = base + corner_offset;
                let opp_raw = ct.opposite[c];
                if opp_raw == NO_CORNER {
                    // Boundary: always a seam.
                    is_edge_on_seam[c] = true;
                    continue;
                }
                let opp_face = opp_raw / 3;
                if visited_faces[opp_face] {
                    continue;
                }
                let bit = seam_bits.get(bit_idx).copied().unwrap_or(false);
                bit_idx += 1;
                if bit {
                    is_edge_on_seam[c] = true;
                    is_edge_on_seam[opp_raw] = true;
                }
            }
        }
        // Mark boundary edges of any unprocessed (start-face or
        // disconnected) faces as seams.
        for c in 0..num_corners {
            if ct.opposite[c] == NO_CORNER {
                is_edge_on_seam[c] = true;
            }
        }

        // Mark vertices on seam edges.
        let mut is_vertex_on_seam = vec![false; num_universal_vertices];
        for c in 0..num_corners {
            if !is_edge_on_seam[c] {
                continue;
            }
            // The two endpoints of the edge opposite corner c are
            // next(c) and previous(c).
            let n_v = usize::from(ct.vertex_idx(next_corner(CornerIdx::from(c))));
            let p_v = usize::from(ct.vertex_idx(previous_corner(CornerIdx::from(c))));
            if n_v < is_vertex_on_seam.len() {
                is_vertex_on_seam[n_v] = true;
            }
            if p_v < is_vertex_on_seam.len() {
                is_vertex_on_seam[p_v] = true;
            }
        }

        // Reconstruct corner_to_vertex by walking each universal vertex's
        // 1-ring. Mirrors `AttributeCornerTable::recompute_vertices`.
        let mut corner_to_vertex = vec![usize::MAX; num_corners];
        let mut left_most_corners: Vec<usize> = Vec::new();
        let mut num_new_vertices = 0usize;

        for v in 0..num_universal_vertices {
            // Use the universal corner table's left_most_corner[v]. For
            // merged-out (phantom) vertices this is NO_CORNER; we skip
            // them so we don't double-count.
            if v >= ct.left_most_corner.len() || ct.left_most_corner[v] == NO_CORNER {
                continue;
            }
            let c = CornerIdx::from(ct.left_most_corner[v]);
            // Sanity: the corner must actually point to v (after alias
            // resolution).
            if usize::from(ct.vertex_idx(c)) != v {
                continue;
            }

            let mut first_vert_id = num_new_vertices;
            num_new_vertices += 1;

            let mut first_c = c;
            // If on seam, swing left until either we hit a boundary or
            // wrap (which shouldn't happen for a true seam vertex).
            if is_vertex_on_seam[v] {
                let mut maybe_curr_c = swing_left(ct, &is_edge_on_seam, first_c);
                while let Some(curr_c) = maybe_curr_c {
                    first_c = curr_c;
                    if curr_c == c {
                        break;
                    }
                    maybe_curr_c = swing_left(ct, &is_edge_on_seam, curr_c);
                }
            }
            corner_to_vertex[usize::from(first_c)] = first_vert_id;
            left_most_corners.push(usize::from(first_c));

            // Swing right, splitting at attribute seams.
            let mut maybe_curr_c = swing_right_universal(ct, first_c);
            while let Some(curr_c) = maybe_curr_c {
                if curr_c == first_c {
                    break;
                }
                // If the corner OPPOSITE to next(curr_c) is across a
                // seam, this corner starts a new attribute vertex.
                let probe = next_corner(curr_c);
                if is_edge_on_seam[usize::from(probe)] {
                    first_vert_id = num_new_vertices;
                    num_new_vertices += 1;
                    left_most_corners.push(usize::from(curr_c));
                }
                corner_to_vertex[usize::from(curr_c)] = first_vert_id;
                maybe_curr_c = swing_right_universal(ct, curr_c);
            }
        }

        Self {
            corner_to_vertex,
            is_edge_on_seam,
            is_vertex_on_seam,
            left_most_corners,
            num_vertices: num_new_vertices,
            opposite_universal: ct.opposite.clone(),
        }
    }
}

#[inline]
fn next_corner(c: CornerIdx) -> CornerIdx {
    let i = usize::from(c);
    let face_base = (i / 3) * 3;
    CornerIdx::from(face_base + (i + 1 - face_base) % 3)
}

#[inline]
fn previous_corner(c: CornerIdx) -> CornerIdx {
    let i = usize::from(c);
    let face_base = (i / 3) * 3;
    CornerIdx::from(face_base + (i + 2 - face_base) % 3)
}

/// Swing right on the UNIVERSAL corner table (ignoring attribute seams).
/// `previous(c) → opposite(prev) → previous(opp)`.
fn swing_right_universal(ct: &DecoderCornerTable, c: CornerIdx) -> Option<CornerIdx> {
    let prev = previous_corner(c);
    let opp_raw = ct.opposite[usize::from(prev)];
    if opp_raw == NO_CORNER {
        return None;
    }
    Some(previous_corner(CornerIdx::from(opp_raw)))
}

/// Swing left on the ATTRIBUTE corner table — stops at seam edges.
fn swing_left(
    ct: &DecoderCornerTable,
    is_edge_on_seam: &[bool],
    c: CornerIdx,
) -> Option<CornerIdx> {
    let nxt = next_corner(c);
    if is_edge_on_seam[usize::from(nxt)] {
        return None;
    }
    let opp_raw = ct.opposite[usize::from(nxt)];
    if opp_raw == NO_CORNER {
        return None;
    }
    Some(next_corner(CornerIdx::from(opp_raw)))
}

impl GenericCornerTable for DecoderAttributeCornerTable {
    fn face_idx_containing(&self, corner: CornerIdx) -> FaceIdx {
        FaceIdx::from(usize::from(corner) / 3)
    }

    fn num_faces(&self) -> usize {
        self.corner_to_vertex.len() / 3
    }

    fn num_corners(&self) -> usize {
        self.corner_to_vertex.len()
    }

    fn num_vertices(&self) -> usize {
        self.num_vertices
    }

    fn point_idx(&self, corner: CornerIdx) -> PointIdx {
        PointIdx::from(self.corner_to_vertex[usize::from(corner)])
    }

    fn vertex_idx(&self, corner: CornerIdx) -> VertexIdx {
        VertexIdx::from(self.corner_to_vertex[usize::from(corner)])
    }

    fn opposite(&self, corner: CornerIdx) -> Option<CornerIdx> {
        if self.is_edge_on_seam[usize::from(corner)] {
            return None;
        }
        let opp = self.opposite_universal[usize::from(corner)];
        if opp == NO_CORNER {
            None
        } else {
            Some(CornerIdx::from(opp))
        }
    }

    fn previous(&self, corner: CornerIdx) -> CornerIdx {
        previous_corner(corner)
    }

    fn next(&self, corner: CornerIdx) -> CornerIdx {
        next_corner(corner)
    }

    fn left_most_corner(&self, vertex: VertexIdx) -> CornerIdx {
        CornerIdx::from(self.left_most_corners[usize::from(vertex)])
    }
}
