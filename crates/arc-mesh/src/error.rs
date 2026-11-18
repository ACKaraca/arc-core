//! The error codes owned by `arc-mesh`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-mesh.ftl`.

/// The error codes owned by `arc-mesh`.
pub mod codes {
    use arc_base::{Category, ErrorCode};
    /// A triangle refers to a vertex index outside the vertex list.
    pub const INDEX_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "mesh.index.out_of_range",
        category: Category::InvalidInput,
        message_id: "mesh-index-out-of-range",
    };
    /// A position or attribute value is NaN or infinite.
    pub const VALUE_NOT_FINITE: ErrorCode = ErrorCode {
        code: "mesh.value.not_finite",
        category: Category::InvalidInput,
        message_id: "mesh-value-not-finite",
    };
    /// An attribute channel does not hold one value per vertex.
    pub const CHANNEL_LENGTH_MISMATCH: ErrorCode = ErrorCode {
        code: "mesh.channel.length_mismatch",
        category: Category::InvalidInput,
        message_id: "mesh-channel-length-mismatch",
    };
    /// An attribute channel has the wrong component type for its key, a tangent handedness other than ±1, an empty or oversized name, or the mesh has too many channels.
    pub const CHANNEL_INVALID: ErrorCode = ErrorCode {
        code: "mesh.channel.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-channel-invalid",
    };
    /// An operation needs an attribute channel the mesh does not have.
    pub const CHANNEL_MISSING: ErrorCode = ErrorCode {
        code: "mesh.channel.missing",
        category: Category::InvalidInput,
        message_id: "mesh-channel-missing",
    };
    /// A face group is empty, out of range, out of order or overlaps another.
    pub const GROUP_INVALID: ErrorCode = ErrorCode {
        code: "mesh.group.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-group-invalid",
    };
    /// A vertex, triangle or index count exceeds 32-bit indices or the limits profile.
    pub const LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "mesh.limit.exceeded",
        category: Category::LimitExceeded,
        message_id: "mesh-limit-exceeded",
    };
    /// A transform is not finite or not invertible.
    pub const TRANSFORM_SINGULAR: ErrorCode = ErrorCode {
        code: "mesh.transform.singular",
        category: Category::InvalidInput,
        message_id: "mesh-transform-singular",
    };
    /// A transform's matrix does not have the bottom row `[0, 0, 0, 1]` of an affine transform.
    pub const TRANSFORM_NOT_AFFINE: ErrorCode = ErrorCode {
        code: "mesh.transform.not_affine",
        category: Category::InvalidInput,
        message_id: "mesh-transform-not-affine",
    };
    /// A result is not representable: a transformed position overflows or a GPU coordinate exceeds `f32`.
    pub const PRECISION_INSUFFICIENT: ErrorCode = ErrorCode {
        code: "mesh.precision.insufficient",
        category: Category::FailedOperation,
        message_id: "mesh-precision-insufficient",
    };
    /// Merged meshes have different attribute channels.
    pub const MERGE_INCOMPATIBLE: ErrorCode = ErrorCode {
        code: "mesh.merge.incompatible",
        category: Category::InvalidInput,
        message_id: "mesh-merge-incompatible",
    };
    /// A tolerance is NaN, infinite or negative.
    pub const TOLERANCE_INVALID: ErrorCode = ErrorCode {
        code: "mesh.tolerance.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-tolerance-invalid",
    };
    /// A crease angle is not a finite number in [0, π].
    pub const CREASE_INVALID: ErrorCode = ErrorCode {
        code: "mesh.crease.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-crease-invalid",
    };
    /// A quantization error bound is not a finite positive number.
    pub const QUANTIZATION_INVALID: ErrorCode = ErrorCode {
        code: "mesh.quantization.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-quantization-invalid",
    };
    /// Encoded mesh data is malformed, truncated or oversized.
    pub const DECODE_INVALID: ErrorCode = ErrorCode {
        code: "mesh.decode.invalid",
        category: Category::CorruptData,
        message_id: "mesh-decode-invalid",
    };
    /// A volume or centroid is requested of a mesh that is not closed and consistently oriented.
    pub const MEASURE_NOT_CLOSED: ErrorCode = ErrorCode {
        code: "mesh.measure.not_closed",
        category: Category::InvalidInput,
        message_id: "mesh-measure-not-closed",
    };
    /// A centroid is requested of a closed mesh that encloses no volume.
    pub const MEASURE_ZERO_VOLUME: ErrorCode = ErrorCode {
        code: "mesh.measure.zero_volume",
        category: Category::InvalidInput,
        message_id: "mesh-measure-zero-volume",
    };
    /// The genus is requested of a mesh with an edge shared by more than two triangles or a vertex whose triangles do not form one fan.
    pub const TOPOLOGY_NON_MANIFOLD: ErrorCode = ErrorCode {
        code: "mesh.topology.non_manifold",
        category: Category::InvalidInput,
        message_id: "mesh-topology-non-manifold",
    };
    /// A ray, query point or distance limit is not finite, or a ray direction is zero.
    pub const QUERY_INVALID: ErrorCode = ErrorCode {
        code: "mesh.query.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-query-invalid",
    };
    /// A query needs a mesh with at least one triangle.
    pub const QUERY_EMPTY: ErrorCode = ErrorCode {
        code: "mesh.query.empty",
        category: Category::InvalidInput,
        message_id: "mesh-query-empty",
    };
    /// A repair option is out of range.
    pub const REPAIR_INVALID: ErrorCode = ErrorCode {
        code: "mesh.repair.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-repair-invalid",
    };
    /// A simplification target or error bound is missing, not finite or negative.
    pub const SIMPLIFY_INVALID: ErrorCode = ErrorCode {
        code: "mesh.simplify.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-simplify-invalid",
    };
    /// A level-of-detail parameter is out of range, or a chain is empty.
    pub const LOD_INVALID: ErrorCode = ErrorCode {
        code: "mesh.lod.invalid",
        category: Category::InvalidInput,
        message_id: "mesh-lod-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        INDEX_OUT_OF_RANGE,
        VALUE_NOT_FINITE,
        CHANNEL_LENGTH_MISMATCH,
        CHANNEL_INVALID,
        CHANNEL_MISSING,
        GROUP_INVALID,
        LIMIT_EXCEEDED,
        TRANSFORM_SINGULAR,
        PRECISION_INSUFFICIENT,
        MERGE_INCOMPATIBLE,
        TOLERANCE_INVALID,
        CREASE_INVALID,
        QUANTIZATION_INVALID,
        DECODE_INVALID,
        TRANSFORM_NOT_AFFINE,
        MEASURE_NOT_CLOSED,
        MEASURE_ZERO_VOLUME,
        TOPOLOGY_NON_MANIFOLD,
        QUERY_INVALID,
        QUERY_EMPTY,
        REPAIR_INVALID,
        SIMPLIFY_INVALID,
        LOD_INVALID,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-mesh.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-mesh.md");

    #[test]
    fn registry_has_no_problems_alone_or_with_its_dependencies() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(arc_geom::error::codes::ALL)
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(
            codes::ALL
                .iter()
                .all(|entry| entry.code.starts_with("mesh."))
        );
    }

    #[test]
    fn english_catalog_contains_every_registered_message() {
        let messages: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
        for entry in codes::ALL {
            assert!(
                messages.contains(entry.message_id),
                "{} is not in the catalog",
                entry.code
            );
        }
    }

    #[test]
    fn every_code_is_documented_in_the_specification() {
        for entry in codes::ALL {
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(
                SPECIFICATION.contains(&cell),
                "{cell} is not in docs/crates/arc-mesh.md"
            );
        }
    }
}
