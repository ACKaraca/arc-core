//! The error codes owned by `arc-gpu`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-gpu.ftl`.

/// The error codes owned by `arc-gpu`.
pub mod codes {
    use arc_base::{Category, ErrorCode};
    /// No adapter matches the settings, or none is available.
    pub const ADAPTER_NOT_FOUND: ErrorCode = ErrorCode {
        code: "gpu.adapter.not_found",
        category: Category::NotFound,
        message_id: "gpu-adapter-not-found",
    };
    /// A settings value is out of range: a sample count other than 1 or 4, a negative or non-finite ambient term or plot scale, an X-ray opacity or tint outside [0, 1], an outline wider than 8 pixels, a non-finite color, or a limit of zero.
    pub const SETTINGS_INVALID: ErrorCode = ErrorCode {
        code: "gpu.settings.invalid",
        category: Category::InvalidInput,
        message_id: "gpu-settings-invalid",
    };
    /// The adapter refused to create a device with the required features and limits.
    pub const DEVICE_REQUEST_FAILED: ErrorCode = ErrorCode {
        code: "gpu.device.request_failed",
        category: Category::FailedOperation,
        message_id: "gpu-device-request-failed",
    };
    /// The device was lost or destroyed; the scene and renderer must be recreated on a new device.
    pub const DEVICE_LOST: ErrorCode = ErrorCode {
        code: "gpu.device.lost",
        category: Category::FailedOperation,
        message_id: "gpu-device-lost",
    };
    /// The device ran out of memory while creating a resource or executing work.
    pub const OUT_OF_MEMORY: ErrorCode = ErrorCode {
        code: "gpu.out_of_memory",
        category: Category::LimitExceeded,
        message_id: "gpu-out-of-memory",
    };
    /// The device reported a validation error: a defect in Arc.
    pub const VALIDATION: ErrorCode = ErrorCode {
        code: "gpu.validation",
        category: Category::Internal,
        message_id: "gpu-validation",
    };
    /// A wait for the device exceeded its bound.
    pub const TIMEOUT: ErrorCode = ErrorCode {
        code: "gpu.timeout",
        category: Category::FailedOperation,
        message_id: "gpu-timeout",
    };
    /// Reading a rendered image back from the device failed.
    pub const READBACK_FAILED: ErrorCode = ErrorCode {
        code: "gpu.readback.failed",
        category: Category::FailedOperation,
        message_id: "gpu-readback-failed",
    };
    /// A count or size exceeds the scene limits or the device limits.
    pub const LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "gpu.limit.exceeded",
        category: Category::LimitExceeded,
        message_id: "gpu-limit-exceeded",
    };
    /// A coordinate, origin, transform, color or width is NaN or infinite.
    pub const VALUE_NOT_FINITE: ErrorCode = ErrorCode {
        code: "gpu.value.not_finite",
        category: Category::InvalidInput,
        message_id: "gpu-value-not-finite",
    };
    /// A render item is empty (no instances, no points) or a line width or miter limit is out of range.
    pub const ITEM_INVALID: ErrorCode = ErrorCode {
        code: "gpu.item.invalid",
        category: Category::InvalidInput,
        message_id: "gpu-item-invalid",
    };
    /// A mesh has no triangles.
    pub const GEOMETRY_INVALID: ErrorCode = ErrorCode {
        code: "gpu.geometry.invalid",
        category: Category::InvalidInput,
        message_id: "gpu-geometry-invalid",
    };
    /// An item or call refers to a geometry handle the scene does not hold.
    pub const GEOMETRY_NOT_FOUND: ErrorCode = ErrorCode {
        code: "gpu.geometry.not_found",
        category: Category::NotFound,
        message_id: "gpu-geometry-not-found",
    };
    /// A geometry is removed while render items still refer to it.
    pub const GEOMETRY_IN_USE: ErrorCode = ErrorCode {
        code: "gpu.geometry.in_use",
        category: Category::Conflict,
        message_id: "gpu-geometry-in-use",
    };
    /// A camera-relative translation or projection cannot be represented in `f32`.
    pub const PRECISION_INSUFFICIENT: ErrorCode = ErrorCode {
        code: "gpu.precision.insufficient",
        category: Category::FailedOperation,
        message_id: "gpu-precision-insufficient",
    };

    /// A pick region is empty or outside the image, or the pick tolerance is negative, not finite or above the line width limit.
    pub const PICK_INVALID: ErrorCode = ErrorCode {
        code: "gpu.pick.invalid",
        category: Category::InvalidInput,
        message_id: "gpu-pick-invalid",
    };
    /// A navigation parameter is out of range: a non-finite value, a zero or negative zoom factor or image size, a negative radius, or a pan anchor at or behind the eye.
    pub const NAVIGATION_INVALID: ErrorCode = ErrorCode {
        code: "gpu.navigation.invalid",
        category: Category::InvalidInput,
        message_id: "gpu-navigation-invalid",
    };
    /// A product extractor panicked; the object keeps its previous items.
    pub const EXTRACTOR_PANICKED: ErrorCode = ErrorCode {
        code: "gpu.extractor.panicked",
        category: Category::Internal,
        message_id: "gpu-extractor-panicked",
    };
    /// A worker or render thread could not be started, or the render thread stopped unexpectedly.
    pub const THREAD_FAILED: ErrorCode = ErrorCode {
        code: "gpu.thread.failed",
        category: Category::FailedOperation,
        message_id: "gpu-thread-failed",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        ADAPTER_NOT_FOUND,
        SETTINGS_INVALID,
        DEVICE_REQUEST_FAILED,
        DEVICE_LOST,
        OUT_OF_MEMORY,
        VALIDATION,
        TIMEOUT,
        READBACK_FAILED,
        LIMIT_EXCEEDED,
        VALUE_NOT_FINITE,
        ITEM_INVALID,
        GEOMETRY_INVALID,
        GEOMETRY_NOT_FOUND,
        GEOMETRY_IN_USE,
        PRECISION_INSUFFICIENT,
        PICK_INVALID,
        NAVIGATION_INVALID,
        EXTRACTOR_PANICKED,
        THREAD_FAILED,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-gpu.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-gpu.md");

    #[test]
    fn registry_has_no_problems_alone_or_with_lower_crates() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(arc_geom::error::codes::ALL)
            .chain(arc_mesh::error::codes::ALL)
            .chain(arc_doc::error::codes::ALL)
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(
            codes::ALL
                .iter()
                .all(|entry| entry.code.starts_with("gpu."))
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
                "{cell} is not in docs/crates/arc-gpu.md"
            );
        }
    }
}
