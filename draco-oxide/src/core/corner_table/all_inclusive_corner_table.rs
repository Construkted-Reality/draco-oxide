use crate::core::{
    corner_table::{attribute_corner_table::AttributeCornerTable, CornerTable, GenericCornerTable},
    shared::{AttributeValueIdx, CornerIdx, FaceIdx, VecVertexIdx},
};

/// All-inclusive corner table that contains the universal corner table and the attribute corner tables (if any).
/// This structure is constructed as a return value of the edgebreaker connectivity encoding, and will be passed to
/// the attribute encoder for read-access.
#[derive(Debug, Clone)]
pub(crate) struct AllInclusiveCornerTable<'faces> {
    universal: CornerTable<'faces>,
    attribute_tables: Vec<AttributeCornerTable>,
}

impl<'faces> AllInclusiveCornerTable<'faces> {
    pub fn new(
        universal: CornerTable<'faces>,
        attribute_tables: Vec<AttributeCornerTable>,
    ) -> Self {
        Self {
            universal,
            attribute_tables,
        }
    }

    pub fn attribute_corner_table<'table>(
        &'table self,
        idx: usize,
    ) -> Option<RefAttributeCornerTable<'faces, 'table>> {
        if idx > 0 {
            let idx = idx - 1;
            if idx < self.attribute_tables.len() {
                Some(RefAttributeCornerTable::new(idx, self))
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn universal_corner_table(&self) -> &CornerTable<'faces> {
        &self.universal
    }

    /// For the attribute with the given attribute-encoder index (the same index
    /// used by `attribute_corner_table`), returns whether its attribute corner
    /// table has no interior (non-boundary) seams. Returns `None` when there is
    /// no attribute corner table for that index (e.g. the position attribute).
    pub fn no_interior_seams(&self, idx: usize) -> Option<bool> {
        if idx == 0 {
            return None;
        }
        self.attribute_tables
            .get(idx - 1)
            .map(|t| t.no_interior_seams())
    }
}

/// Reference to an attribute corner table.
/// Mostly used to read-access the attribute corner table when encoding attributes.
#[derive(Debug, Clone)]
pub(crate) struct RefAttributeCornerTable<'faces, 'table> {
    // Cached references resolved once at construction. Navigation (`vertex_idx`,
    // `next`, `opposite`, …) is called millions of times during attribute
    // prediction; re-doing `attribute_tables.get(idx).unwrap()` per call was a
    // measured ~10% of encode (C++ holds a direct pointer and inlines these).
    attr_table: &'table AttributeCornerTable,
    universal: &'table CornerTable<'faces>,
}

impl<'faces, 'table> RefAttributeCornerTable<'faces, 'table> {
    pub fn new(idx: usize, corner_table: &'table AllInclusiveCornerTable<'faces>) -> Self {
        let attr_table = corner_table
            .attribute_tables
            .get(idx)
            .expect("attribute corner table index out of range");
        Self {
            attr_table,
            universal: &corner_table.universal,
        }
    }
}

impl<'faces, 'table> GenericCornerTable for RefAttributeCornerTable<'faces, 'table> {
    #[inline]
    fn face_idx_containing(&self, corner: CornerIdx) -> FaceIdx {
        // The face index is the same as in the universal corner table
        self.universal.face_idx_containing(corner)
    }

    #[inline]
    fn num_faces(&self) -> usize {
        // number of faces is the same as the number of faces in the universal corner table
        self.universal.num_faces()
    }

    #[inline]
    fn num_corners(&self) -> usize {
        // number of corners is the same as the number of corners in the universal corner table
        self.universal.num_corners()
    }
    #[inline]
    fn num_vertices(&self) -> usize {
        self.attr_table.num_vertices()
    }
    #[inline]
    fn point_idx(&self, corner: CornerIdx) -> crate::core::shared::PointIdx {
        self.universal.point_idx(corner)
    }
    #[inline]
    fn vertex_idx(&self, corner: CornerIdx) -> crate::core::shared::VertexIdx {
        self.attr_table.vertex_idx(corner)
    }
    #[inline]
    fn next(&self, c: CornerIdx) -> CornerIdx {
        self.attr_table.next(c, self.universal)
    }
    #[inline]
    fn previous(&self, c: CornerIdx) -> CornerIdx {
        self.attr_table.previous(c, self.universal)
    }
    #[inline]
    fn opposite(&self, c: CornerIdx) -> Option<CornerIdx> {
        self.attr_table.opposite(c, self.universal)
    }
    #[inline]
    fn left_most_corner(&self, vertex: crate::core::shared::VertexIdx) -> CornerIdx {
        self.attr_table.left_most_corner(vertex)
    }
    #[inline]
    fn vertex_to_attribute_map(&self) -> Option<&VecVertexIdx<AttributeValueIdx>> {
        Some(self.attr_table.get_vertex_to_attribute_map())
    }
}
