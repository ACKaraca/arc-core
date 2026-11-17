//! The coordinate reference record (CORE-BASE-023).

use serde::{Deserialize, Serialize};

use crate::math::{Point3, Vector3};

/// Where a document sits in its project: plain data every document carries.
///
/// Model geometry is stored in *local* coordinates, close to the origin so that `f64` keeps its
/// precision. *Project* coordinates are the shared frame of the site (easting, northing,
/// elevation), in meters. The local frame is the project frame rotated about the vertical axis
/// by [`rotation_to_grid_north`](Self::rotation_to_grid_north) and moved to
/// [`local_origin`](Self::local_origin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrsRecord {
    /// The project coordinates of the local origin, in meters.
    pub local_origin: Point3,
    /// The project coordinates of the survey point, the surveyed reference of the site, in
    /// meters.
    pub survey_point: Point3,
    /// The counterclockwise angle, in radians and seen from above, from the local Y axis to grid
    /// north.
    pub rotation_to_grid_north: f64,
    /// The identifier of the coordinate reference system of the project frame, for example an
    /// EPSG code; `None` when the project is not georeferenced.
    pub crs_id: Option<String>,
    /// The identifier of the vertical datum of elevations; `None` when unknown.
    pub vertical_datum: Option<String>,
}

impl Default for CrsRecord {
    /// A record whose local and project coordinates coincide.
    fn default() -> Self {
        Self {
            local_origin: Point3::origin(),
            survey_point: Point3::origin(),
            rotation_to_grid_north: 0.0,
            crs_id: None,
            vertical_datum: None,
        }
    }
}

impl CrsRecord {
    /// Transforms a point from local to project coordinates.
    #[must_use]
    pub fn local_to_project(&self, local: &Point3) -> Point3 {
        // Grid north lies `rotation` counterclockwise of local Y, so local axes are the project
        // axes turned clockwise by it.
        let (sin, cos) = self.rotation_to_grid_north.sin_cos();
        let rotated = Vector3::new(
            cos * local.x + sin * local.y,
            -sin * local.x + cos * local.y,
            local.z,
        );
        self.local_origin + rotated
    }

    /// Transforms a point from project to local coordinates; the inverse of
    /// [`local_to_project`](Self::local_to_project).
    #[must_use]
    pub fn project_to_local(&self, project: &Point3) -> Point3 {
        let (sin, cos) = self.rotation_to_grid_north.sin_cos();
        let offset = project - self.local_origin;
        Point3::new(
            cos * offset.x - sin * offset.y,
            sin * offset.x + cos * offset.y,
            offset.z,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_PI_2;

    use proptest::prelude::{prop_assert, proptest};

    use super::*;

    fn near(a: &Point3, b: &Point3, tolerance: f64) -> bool {
        (a - b).norm() <= tolerance
    }

    #[test]
    fn default_record_is_the_identity() {
        let record = CrsRecord::default();
        let point = Point3::new(1.0, 2.0, 3.0);
        assert_eq!(record.local_to_project(&point), point);
        assert_eq!(record.project_to_local(&point), point);
    }

    #[test]
    fn local_origin_translates_exactly() {
        let record = CrsRecord {
            local_origin: Point3::new(500_000.0, 4_500_000.0, 120.0),
            ..CrsRecord::default()
        };
        let project = record.local_to_project(&Point3::new(1.5, -2.25, 0.5));
        assert_eq!(project, Point3::new(500_001.5, 4_499_997.75, 120.5));
        assert_eq!(
            record.project_to_local(&project),
            Point3::new(1.5, -2.25, 0.5)
        );
    }

    #[test]
    fn rotation_turns_local_y_away_from_grid_north() {
        // Grid north is a quarter turn counterclockwise of local Y, so it points along local -X
        // and local Y points to grid east.
        let record = CrsRecord {
            rotation_to_grid_north: FRAC_PI_2,
            ..CrsRecord::default()
        };
        let east = record.local_to_project(&Point3::new(0.0, 1.0, 0.0));
        assert!(near(&east, &Point3::new(1.0, 0.0, 0.0), 1e-15), "{east}");
        let north = record.local_to_project(&Point3::new(-1.0, 0.0, 7.0));
        assert!(near(&north, &Point3::new(0.0, 1.0, 7.0), 1e-15), "{north}");
    }

    #[test]
    fn record_round_trips_through_its_stored_form() {
        let record = CrsRecord {
            local_origin: Point3::new(1.0, 2.0, 3.0),
            survey_point: Point3::new(4.0, 5.0, 6.0),
            rotation_to_grid_north: 0.125,
            crs_id: Some("EPSG:5254".to_owned()),
            vertical_datum: Some("EPSG:5773".to_owned()),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<CrsRecord>(&json).unwrap(), record);
    }

    proptest! {
        #[test]
        fn project_to_local_inverts_local_to_project(
            origin in proptest::array::uniform3(-1e7_f64..1e7),
            local in proptest::array::uniform3(-1e4_f64..1e4),
            rotation in -7.0_f64..7.0,
        ) {
            let record = CrsRecord {
                local_origin: Point3::from(origin),
                rotation_to_grid_north: rotation,
                ..CrsRecord::default()
            };
            let local = Point3::from(local);
            let back = record.project_to_local(&record.local_to_project(&local));
            // Limited by the precision of the project coordinates (about 1e7 m).
            prop_assert!(near(&back, &local, 1e-8), "{local} -> {back}");
        }
    }
}
