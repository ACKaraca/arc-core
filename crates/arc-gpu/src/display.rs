//! Display overrides (CORE-GPU-016) and section planes and box (CORE-GPU-024).
//!
//! Both are per-view inputs of a frame ([`crate::FrameExtras`]): they are uploaded in the view
//! uniform and per-frame bind groups and never write a scene buffer, so changing them needs no
//! re-extraction.
//!
//! # Overrides
//!
//! A [`DisplayOverrides`] changes how objects look in one view:
//!
//! - **By object.** [`DisplayOverrides::objects`] gives an object a [`OverrideStyle`]. The CPU
//!   chooses the draws, so a hidden object (and an object an isolation excludes) is not drawn
//!   in any pass: not shaded, tinted, outlined or picked.
//! - **By category.** An instance carries a category from 0 to 15 in the bits 24 to 27 of its
//!   flags ([`category_flags`]). [`DisplayOverrides::categories`] gives a category a style that
//!   the vertex shader looks up per instance, so one draw can mix categories. An object's own
//!   style replaces the style of its instances' categories.
//! - **By rule.** A rule (a property, a layer, a type) is evaluated by the caller into object
//!   identifiers or a category; the renderer does not know document properties.
//! - **Isolation** shows only the listed objects; the others are hidden or ghosted.
//! - **Ghosting** draws an object at [`DisplayOverrides::ghost_opacity`] and does not pick it.
//!
//! A style's color replaces the instance color of mesh faces and of the edges drawn in the
//! instance color; its opacity blends mesh faces and edges (faces with an opacity below 1 are
//! drawn after the opaque ones without depth writes, in the modes that draw lit or unlit
//! colored faces; the X-ray mode multiplies its own opacity). Hiding and isolation also apply
//! to line items; color, opacity and ghosting do not.
//!
//! # Sections
//!
//! A [`Section`] clips mesh faces and edges by up to six planes and one axis-aligned box. A
//! fragment is discarded when it lies on the normal side of any plane or outside the box, so
//! the clipping is the same in every backend. Overlays are not clipped; line items are not
//! clipped. Instances flagged [`FLAG_SOLID`] are closed solids: while the section has a cap
//! color, their back faces that remain visible after clipping are drawn unlit in that color,
//! which fills the cut. The cap is the nearest back face behind the cut, not a polygon on the
//! cutting plane, so its depth is that of the back face. A [`CapHatch`] draws parallel lines in
//! a second color over the cap; the lines are in screen space, so their spacing is in pixels.

use std::collections::{BTreeMap, HashMap, HashSet};

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Error, ObjectId, Result};

use crate::error::codes;
use crate::scene::{ItemEntry, Scene};

/// The number of categories an instance can carry.
pub const CATEGORIES: usize = 16;
/// The bit position of the category in the flags of an instance.
pub const CATEGORY_SHIFT: u32 = 24;
/// The instance flag of a closed solid that sections cap.
pub const FLAG_SOLID: u32 = 1 << 4;
/// The largest number of section planes of a view.
pub const MAX_SECTION_PLANES: usize = 6;

/// `flags` with the category `category` (0 to 15; larger values wrap) set.
#[must_use]
pub fn category_flags(flags: u32, category: u8) -> u32 {
    (flags & !(0xF << CATEGORY_SHIFT)) | (u32::from(category & 0xF) << CATEGORY_SHIFT)
}

/// How an object or category looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverrideStyle {
    /// Replaces the color of faces and instance-colored edges; the alpha is kept.
    pub color: Option<Color>,
    /// Multiplies the opacity, in [0, 1]; 1 is opaque.
    pub opacity: f32,
    /// Draws the object at the view's ghost opacity and does not pick it.
    pub ghost: bool,
    /// Does not draw the object at all.
    pub hidden: bool,
}

impl OverrideStyle {
    /// No change.
    pub const NONE: Self = Self {
        color: None,
        opacity: 1.0,
        ghost: false,
        hidden: false,
    };

    /// Hidden.
    #[must_use]
    pub const fn hidden() -> Self {
        Self {
            hidden: true,
            ..Self::NONE
        }
    }

    /// Ghosted.
    #[must_use]
    pub const fn ghost() -> Self {
        Self {
            ghost: true,
            ..Self::NONE
        }
    }

    /// Colored.
    #[must_use]
    pub const fn colored(color: Color) -> Self {
        Self {
            color: Some(color),
            ..Self::NONE
        }
    }

    /// Transparent at `opacity`.
    #[must_use]
    pub const fn transparent(opacity: f32) -> Self {
        Self {
            opacity,
            ..Self::NONE
        }
    }

    fn validate(&self) -> Result<()> {
        let color_ok = self
            .color
            .is_none_or(|c| [c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite()));
        if color_ok && (0.0..=1.0).contains(&self.opacity) {
            Ok(())
        } else {
            Err(Error::new(codes::SETTINGS_INVALID))
        }
    }
}

/// What happens to the objects an isolation does not list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Others {
    /// They are not drawn.
    Hide,
    /// They are ghosted.
    Ghost,
}

/// Show only `objects`.
#[derive(Debug, Clone, PartialEq)]
pub struct Isolation {
    /// The objects that stay as they are.
    pub objects: Vec<ObjectId>,
    /// What happens to every other object.
    pub others: Others,
}

/// The per-view display overrides; see the module documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayOverrides {
    /// Styles by object; the last entry of an object wins.
    pub objects: Vec<(ObjectId, OverrideStyle)>,
    /// Styles by category, 0 to 15; the last entry of a category wins.
    pub categories: Vec<(u8, OverrideStyle)>,
    /// The isolation, if any.
    pub isolation: Option<Isolation>,
    /// The opacity of ghosted objects, in [0, 1].
    pub ghost_opacity: f32,
}

impl Default for DisplayOverrides {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            categories: Vec::new(),
            isolation: None,
            ghost_opacity: 0.15,
        }
    }
}

impl DisplayOverrides {
    /// Checks every value.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` for an opacity outside [0, 1], a non-finite color or a category
    /// above 15; `gpu.limit.exceeded` for more entries than `max_objects`.
    pub fn validate(&self, max_objects: u32) -> Result<()> {
        let max = usize::try_from(max_objects).unwrap_or(usize::MAX);
        let listed = self.objects.len()
            + self.isolation.as_ref().map_or(0, |i| i.objects.len())
            + self.categories.len();
        if listed > max {
            return Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_objects"));
        }
        if !(0.0..=1.0).contains(&self.ghost_opacity)
            || self
                .categories
                .iter()
                .any(|(c, _)| usize::from(*c) >= CATEGORIES)
        {
            return Err(Error::new(codes::SETTINGS_INVALID));
        }
        self.objects
            .iter()
            .map(|(_, s)| s)
            .chain(self.categories.iter().map(|(_, s)| s))
            .try_for_each(OverrideStyle::validate)
    }
}

/// The GPU form of a style: color (alpha 1 when it replaces the color), then opacity, hidden,
/// ghost and a zero.
pub(crate) type GpuStyle = [[f32; 4]; 2];

fn gpu_style(style: &OverrideStyle, ghost_opacity: f32) -> GpuStyle {
    let c = style.color.map_or([0.0; 4], |c| [c.r, c.g, c.b, 1.0]);
    let opacity = if style.ghost {
        style.opacity.min(ghost_opacity)
    } else {
        style.opacity
    };
    [
        c,
        [
            opacity,
            f32::from(u8::from(style.hidden)),
            f32::from(u8::from(style.ghost)),
            0.0,
        ],
    ]
}

/// The identity style.
const IDENTITY: GpuStyle = [[0.0; 4], [1.0, 0.0, 0.0, 0.0]];

/// Overrides resolved for one frame.
#[derive(Debug, Default)]
pub(crate) struct Resolved {
    /// The distinct styles of objects.
    styles: Vec<OverrideStyle>,
    by_object: HashMap<ObjectId, usize>,
    isolated: Option<(HashSet<ObjectId>, Others)>,
    ghost_opacity: f32,
    /// The category table of the view uniform.
    pub(crate) table: [GpuStyle; CATEGORIES],
    /// Whether anything is drawn with an opacity below 1.
    pub(crate) translucent: bool,
}

impl Resolved {
    pub(crate) fn new(overrides: Option<&DisplayOverrides>, max_objects: u32) -> Result<Self> {
        let mut resolved = Self {
            table: [IDENTITY; CATEGORIES],
            ghost_opacity: 0.15,
            ..Self::default()
        };
        let Some(o) = overrides else {
            return Ok(resolved);
        };
        o.validate(max_objects)?;
        resolved.ghost_opacity = o.ghost_opacity;
        for (category, style) in &o.categories {
            if let Some(slot) = resolved.table.get_mut(usize::from(*category)) {
                *slot = gpu_style(style, o.ghost_opacity);
            }
            resolved.translucent |= gpu_style(style, o.ghost_opacity)[1][0] < 1.0;
        }
        for (object, style) in &o.objects {
            resolved.set_object(*object, *style);
        }
        if let Some(isolation) = &o.isolation {
            resolved.translucent |= isolation.others == Others::Ghost && o.ghost_opacity < 1.0;
            resolved.isolated = Some((
                isolation.objects.iter().copied().collect(),
                isolation.others,
            ));
        }
        Ok(resolved)
    }

    fn set_object(&mut self, object: ObjectId, style: OverrideStyle) {
        let at = self
            .styles
            .iter()
            .position(|s| *s == style)
            .unwrap_or_else(|| {
                self.styles.push(style);
                self.styles.len() - 1
            });
        self.translucent |= gpu_style(&style, self.ghost_opacity)[1][0] < 1.0;
        self.by_object.insert(object, at);
    }

    /// The style of an object: its own, with ghosting added when an isolation ghosts it.
    fn style_of(&self, object: &ObjectId) -> Option<OverrideStyle> {
        let own = self
            .by_object
            .get(object)
            .and_then(|at| self.styles.get(*at).copied());
        match &self.isolated {
            Some((set, Others::Ghost)) if !set.contains(object) => Some(OverrideStyle {
                ghost: true,
                ..own.unwrap_or(OverrideStyle::NONE)
            }),
            _ => own,
        }
    }

    /// Whether an object is drawn.
    pub(crate) fn visible(&self, object: &ObjectId) -> bool {
        let isolated_out = self
            .isolated
            .as_ref()
            .is_some_and(|(set, others)| *others == Others::Hide && !set.contains(object));
        !isolated_out && !self.style_of(object).is_some_and(|s| s.hidden)
    }

    fn pickable(&self, object: &ObjectId) -> bool {
        self.visible(object) && !self.style_of(object).is_some_and(|s| s.ghost)
    }

    /// The items to draw grouped by object style (`None` for no style), without the objects
    /// that are not drawn, or, for picking, not pickable.
    pub(crate) fn groups<'a>(
        &self,
        scene: &'a Scene,
        picking: bool,
    ) -> Vec<(Option<GpuStyle>, Vec<&'a ItemEntry>)> {
        let mut groups: BTreeMap<Option<usize>, Vec<&'a ItemEntry>> = BTreeMap::new();
        let mut distinct: Vec<OverrideStyle> = Vec::new();
        for (object, item) in scene.items_with_object() {
            let ok = if picking {
                self.pickable(object)
            } else {
                self.visible(object)
            };
            if !ok || !item.visible {
                continue;
            }
            let key = self.style_of(object).map(|style| {
                distinct
                    .iter()
                    .position(|s| *s == style)
                    .unwrap_or_else(|| {
                        distinct.push(style);
                        distinct.len() - 1
                    })
            });
            groups.entry(key).or_default().push(item);
        }
        groups
            .into_iter()
            .map(|(key, items)| {
                let style = key
                    .and_then(|k| distinct.get(k))
                    .map(|s| gpu_style(s, self.ghost_opacity));
                (style, items)
            })
            .collect()
    }
}

/// A clip plane. A point `x` is removed when `normal · (x − point) > 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionPlane {
    /// A point on the plane.
    pub point: Point3,
    /// The direction toward the removed side; not zero.
    pub normal: Vector3,
}

/// An axis-aligned section box; everything outside is removed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionBox {
    /// The corner with the smallest coordinates.
    pub min: Point3,
    /// The corner with the largest coordinates.
    pub max: Point3,
}

/// Parallel lines drawn over the caps of a section (CORE-GPU-024). The lines are in screen
/// space: their spacing and width are in pixels, so their density follows the view. Without
/// caps the hatch has no effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapHatch {
    /// The color of the lines. Its alpha is ignored: lines are drawn opaque over the cap.
    pub color: Color,
    /// The direction of the lines in degrees, counterclockwise as seen on the screen, from the
    /// horizontal. Must be finite.
    pub angle_deg: f64,
    /// The distance between neighboring lines in pixels, above 0 and at most 4096.
    pub spacing_px: f32,
    /// The width of a line in pixels, above 0 and below the spacing.
    pub width_px: f32,
}

/// The largest hatch spacing in pixels.
pub const MAX_HATCH_SPACING_PX: f32 = 4096.0;

/// The section of a view; see the module documentation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    /// Up to [`MAX_SECTION_PLANES`] planes.
    pub planes: Vec<SectionPlane>,
    /// The section box.
    pub volume: Option<SectionBox>,
    /// The unlit color of caps on cut solids; `None` draws no caps.
    pub cap_color: Option<Color>,
    /// The hatch over the caps; `None` draws the caps in one color.
    pub hatch: Option<CapHatch>,
}

/// The section as the view uniform holds it, relative to the translation origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SectionUniform {
    /// Plane count, box active, caps active, hatch active (caps only).
    pub(crate) counts: [f32; 4],
    pub(crate) cap_color: [f32; 4],
    pub(crate) planes: [[f32; 4]; MAX_SECTION_PLANES],
    pub(crate) box_min: [f32; 4],
    pub(crate) box_max: [f32; 4],
    /// The normal of the hatch lines in pixels (x, y), the spacing and the width in pixels.
    pub(crate) hatch: [f32; 4],
    /// The hatch color, alpha 1.
    pub(crate) hatch_color: [f32; 4],
}

impl SectionUniform {
    pub(crate) const NONE: Self = Self {
        counts: [0.0; 4],
        cap_color: [0.0; 4],
        planes: [[0.0; 4]; MAX_SECTION_PLANES],
        box_min: [0.0; 4],
        box_max: [0.0; 4],
        hatch: [0.0; 4],
        hatch_color: [0.0; 4],
    };
}

fn narrow(v: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Narrowing to the GPU's precision is the intent.
    let narrowed = v as f32;
    narrowed
}

impl Section {
    /// Whether the section removes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.planes.is_empty() && self.volume.is_none()
    }

    /// Resolves the section against the translation origin `eye`, in `f64`.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` for more than six planes, a zero normal or an inverted box;
    /// `gpu.value.not_finite` for a value that is not finite.
    pub(crate) fn resolve(&self, eye: &Point3) -> Result<SectionUniform> {
        let mut out = SectionUniform::NONE;
        if self.planes.len() > MAX_SECTION_PLANES {
            return Err(Error::new(codes::SETTINGS_INVALID));
        }
        for (slot, plane) in out.planes.iter_mut().zip(&self.planes) {
            let finite = plane
                .point
                .iter()
                .chain(plane.normal.iter())
                .all(|v| v.is_finite());
            if !finite {
                return Err(Error::new(codes::VALUE_NOT_FINITE));
            }
            let n = crate::scene::unit(&plane.normal)
                .ok_or_else(|| Error::new(codes::SETTINGS_INVALID))?;
            let distance = n.dot(&(plane.point - eye));
            *slot = [narrow(n.x), narrow(n.y), narrow(n.z), narrow(distance)];
        }
        out.counts[0] = narrow(self.planes.len() as f64);
        if let Some(b) = &self.volume {
            if !b.min.iter().chain(b.max.iter()).all(|v| v.is_finite()) {
                return Err(Error::new(codes::VALUE_NOT_FINITE));
            }
            if (0..3).any(|k| b.min[k] > b.max[k]) {
                return Err(Error::new(codes::SETTINGS_INVALID));
            }
            let (lo, hi) = (b.min - eye, b.max - eye);
            out.box_min = [narrow(lo.x), narrow(lo.y), narrow(lo.z), 0.0];
            out.box_max = [narrow(hi.x), narrow(hi.y), narrow(hi.z), 0.0];
            out.counts[1] = 1.0;
        }
        if let (Some(c), false) = (self.cap_color, self.is_empty()) {
            if ![c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite()) {
                return Err(Error::new(codes::VALUE_NOT_FINITE));
            }
            out.cap_color = [c.r, c.g, c.b, 1.0];
            out.counts[2] = 1.0;
        }
        if let Some(h) = self.hatch {
            let c = h.color;
            if !([c.r, c.g, c.b].iter().all(|v| v.is_finite()) && h.angle_deg.is_finite()) {
                return Err(Error::new(codes::VALUE_NOT_FINITE));
            }
            if !(h.spacing_px.is_finite() && h.width_px.is_finite()) {
                return Err(Error::new(codes::VALUE_NOT_FINITE));
            }
            if !(h.spacing_px > 0.0
                && h.spacing_px <= MAX_HATCH_SPACING_PX
                && h.width_px > 0.0
                && h.width_px < h.spacing_px)
            {
                return Err(Error::new(codes::SETTINGS_INVALID));
            }
            // The lines are the level sets of the screen position along the normal of their
            // direction, which is counterclockwise on the screen (y points down the image).
            let angle = h.angle_deg.to_radians();
            out.hatch = [
                narrow(angle.sin()),
                narrow(angle.cos()),
                h.spacing_px,
                h.width_px,
            ];
            out.hatch_color = [c.r, c.g, c.b, 1.0];
            out.counts[3] = f32::from(u8::from(out.counts[2] > 0.0));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests fail by panicking")]

    use super::*;

    #[test]
    fn categories_live_in_the_top_flag_bits_and_wrap() {
        assert_eq!(category_flags(FLAG_SOLID, 5), FLAG_SOLID | (5 << 24));
        assert_eq!(category_flags(5 << 24, 2), 2 << 24);
        assert_eq!(category_flags(0, 17), 1 << 24);
    }

    #[test]
    fn invalid_overrides_and_sections_are_rejected() {
        let bad_opacity = DisplayOverrides {
            categories: vec![(0, OverrideStyle::transparent(1.5))],
            ..DisplayOverrides::default()
        };
        assert_eq!(
            bad_opacity.validate(10).unwrap_err().code(),
            codes::SETTINGS_INVALID.code
        );
        let bad_category = DisplayOverrides {
            categories: vec![(16, OverrideStyle::hidden())],
            ..DisplayOverrides::default()
        };
        assert_eq!(
            bad_category.validate(10).unwrap_err().code(),
            codes::SETTINGS_INVALID.code
        );
        let plane = SectionPlane {
            point: Point3::origin(),
            normal: Vector3::z(),
        };
        let seven = Section {
            planes: vec![plane; 7],
            ..Section::default()
        };
        assert_eq!(
            seven.resolve(&Point3::origin()).unwrap_err().code(),
            codes::SETTINGS_INVALID.code
        );
        let zero = Section {
            planes: vec![SectionPlane {
                normal: Vector3::zeros(),
                ..plane
            }],
            ..Section::default()
        };
        assert_eq!(
            zero.resolve(&Point3::origin()).unwrap_err().code(),
            codes::SETTINGS_INVALID.code
        );
    }

    #[test]
    fn plane_distance_is_taken_from_the_translation_origin() {
        let section = Section {
            planes: vec![SectionPlane {
                point: Point3::new(0.0, 0.0, 10_000_000.5),
                normal: Vector3::new(0.0, 0.0, 2.0),
            }],
            ..Section::default()
        };
        let u = section
            .resolve(&Point3::new(1.0, 2.0, 10_000_000.0))
            .unwrap();
        assert_eq!(u.planes[0], [0.0, 0.0, 1.0, 0.5]);
        assert_eq!(u.counts[0], 1.0);
    }

    #[test]
    fn hatch_settings_are_validated_when_the_section_resolves() {
        let hatch = |spacing_px: f32, width_px: f32, angle_deg: f64| CapHatch {
            color: Color::WHITE,
            angle_deg,
            spacing_px,
            width_px,
        };
        let with = |h: CapHatch| Section {
            cap_color: Some(Color::WHITE),
            planes: vec![SectionPlane {
                point: Point3::origin(),
                normal: Vector3::z(),
            }],
            hatch: Some(h),
            ..Section::default()
        };
        let code = |h: CapHatch| with(h).resolve(&Point3::origin()).unwrap_err().code();
        assert_eq!(code(hatch(0.0, 1.0, 0.0)), codes::SETTINGS_INVALID.code);
        assert_eq!(code(hatch(4.0, 4.0, 0.0)), codes::SETTINGS_INVALID.code);
        assert_eq!(code(hatch(4.0, 0.0, 0.0)), codes::SETTINGS_INVALID.code);
        assert_eq!(code(hatch(5000.0, 1.0, 0.0)), codes::SETTINGS_INVALID.code);
        assert_eq!(
            code(hatch(4.0, 1.0, f64::NAN)),
            codes::VALUE_NOT_FINITE.code
        );
        let uniform = with(hatch(6.0, 2.0, 90.0))
            .resolve(&Point3::origin())
            .unwrap();
        // At 90 degrees the lines are vertical: the normal is the image's horizontal axis.
        assert!((uniform.hatch[0] - 1.0).abs() < 1e-6 && uniform.hatch[1].abs() < 1e-6);
        assert_eq!(uniform.hatch[2..], [6.0, 2.0]);
        assert_eq!(uniform.counts[3], 1.0);
        // Without caps the hatch is inactive.
        let no_caps = Section {
            cap_color: None,
            ..with(hatch(6.0, 2.0, 0.0))
        };
        assert_eq!(no_caps.resolve(&Point3::origin()).unwrap().counts[3], 0.0);
    }
}
