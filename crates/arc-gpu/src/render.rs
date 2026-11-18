//! The frame, headless rendering, shading modes, highlights, overlays and the identifier pass
//! (CORE-GPU-007, 008, 014, 022, 023, 025, 031).
//!
//! A frame is the list of [`Pass`]es of its [`ShadingMode`] encoded in order into one render
//! pass on an offscreen target, preceded by a highlight mask pass when objects are
//! highlighted. [`Renderer::render_to_image`] renders a [`Scene`] seen through a
//! [`ViewFrame`] to an [`Image`] without a window, on any adapter;
//! [`Renderer::render_with`] adds highlights and an overlay; [`Renderer::pick`] runs the
//! identifier pass for a region of interest.
//!
//! Shading modes, highlights and overlays are per-frame inputs: they are uploaded in a
//! uniform buffer and per-frame buffers created for the frame, never in the scene's pools, so
//! changing them writes no scene buffer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use arc_base::math::Matrix4;
use arc_base::{Color, Error, ObjectId, Result};
use arc_geom::view::ViewFrame;

use crate::adapter::Gpu;
use crate::camera::{camera_data, depth_origin, region_window, sphere_depth};
use crate::error::codes;
use crate::overlay::{FILL_VERTEX_STRIDE, Overlay, Resolved};
use crate::scene::{
    ItemEntry, ItemGpu, ItemKey, MESH_INSTANCE_STRIDE, SEGMENT_STRIDE, Scene, TRANSLATION_STRIDE,
    VERTEX_STRIDE,
};

/// The color target format: 8-bit sRGB with alpha; blending happens in linear space.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The depth target format.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The identifier target format.
pub const PICK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// The highlight mask format.
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Uint;
/// How long reading an image back may take.
pub const READBACK_TIMEOUT: Duration = Duration::from_secs(60);
/// The widest highlight outline in pixels.
pub const MAX_OUTLINE_PX: u32 = 8;

/// Identifier-pass values of line segments have this bit set.
const LINE_BIT: u32 = 0x8000_0000;
/// Mask codes: selection and hover.
const MASK_SELECTED: u32 = 1;
const MASK_HOVERED: u32 = 2;

/// A pass of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Mesh faces: lit or unlit with depth writes, depth only, or blended without depth
    /// writes, as the shading mode says.
    Meshes,
    /// Feature edges of meshes, one pixel wide, depth-tested against the faces.
    Edges,
    /// Lines, depth-tested against the meshes, blended by coverage.
    Lines,
    /// The highlight tint of selected and hovered objects.
    Tint,
    /// The highlight outline from the mask pass.
    Outline,
    /// The overlay: fills, then lines, without depth testing.
    Overlay,
}

/// How meshes are drawn. Lines draw the same in every mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ShadingMode {
    /// Feature edges in the instance color, no faces, no hidden-line removal.
    Wireframe,
    /// Feature edges in the instance color; faces only hide what is behind them.
    HiddenLine,
    /// Faces lit by the headlight.
    #[default]
    Shaded,
    /// Lit faces with feature edges in [`RenderSettings::edge_color`].
    ShadedWithEdges,
    /// Unlit faces in their base color.
    ConsistentColors,
    /// Lit faces at [`RenderSettings::xray_opacity`] without depth writes, with feature edges
    /// in the instance color.
    XRay,
}

impl ShadingMode {
    /// Every mode.
    pub const ALL: [Self; 6] = [
        Self::Wireframe,
        Self::HiddenLine,
        Self::Shaded,
        Self::ShadedWithEdges,
        Self::ConsistentColors,
        Self::XRay,
    ];

    /// The passes of a frame in this mode, in order.
    #[must_use]
    pub fn passes(self) -> &'static [Pass] {
        use Pass::{Edges, Lines, Meshes, Outline, Overlay, Tint};
        match self {
            Self::Wireframe => &[Edges, Lines, Tint, Outline, Overlay],
            Self::Shaded | Self::ConsistentColors => &[Meshes, Lines, Tint, Outline, Overlay],
            Self::HiddenLine | Self::ShadedWithEdges | Self::XRay => {
                &[Meshes, Edges, Lines, Tint, Outline, Overlay]
            }
        }
    }

    fn faces(self) -> Option<Kind> {
        match self {
            Self::Wireframe => None,
            Self::HiddenLine => Some(Kind::MeshDepth),
            Self::XRay => Some(Kind::MeshXray),
            Self::Shaded | Self::ShadedWithEdges | Self::ConsistentColors => Some(Kind::Mesh),
        }
    }
}

/// The settings of one rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    /// The requested multisample count: 1 or 4. Adapters without four-sample support render
    /// with one sample (reported as [`crate::ReducedFeature::Multisampling`]).
    pub sample_count: u32,
    /// Whether triangles facing away from the eye are culled. Front faces are counterclockwise.
    pub cull_back_faces: bool,
    /// Whether line edges are antialiased by coverage.
    pub line_smoothing: bool,
    /// The background color.
    pub background: Color,
    /// The ambient term of the headlight model, in [0, 1].
    pub ambient: f32,
    /// Model units per paper millimetre, for paper widths; positive.
    pub plot_scale: f64,
    /// The shading mode.
    pub mode: ShadingMode,
    /// The edge color of [`ShadingMode::ShadedWithEdges`].
    pub edge_color: Color,
    /// The face opacity of [`ShadingMode::XRay`], in [0, 1].
    pub xray_opacity: f32,
    /// The tint and outline color of selected objects.
    pub selection_color: Color,
    /// The tint and outline color of the hovered object.
    pub hover_color: Color,
    /// The tint strength: the alpha the highlight color is blended with, in [0, 1].
    pub tint: f32,
    /// The outline width in pixels, at most [`MAX_OUTLINE_PX`]; 0 draws no outline.
    pub outline_px: u32,
}

impl RenderSettings {
    /// The default settings.
    pub const DEFAULT: Self = Self {
        sample_count: 1,
        cull_back_faces: false,
        line_smoothing: true,
        background: Color::BLACK,
        ambient: 0.25,
        plot_scale: 1.0,
        mode: ShadingMode::Shaded,
        edge_color: Color::BLACK,
        xray_opacity: 0.3,
        selection_color: Color::new(0.0, 0.45, 1.0, 1.0),
        hover_color: Color::new(1.0, 0.6, 0.0, 1.0),
        tint: 0.35,
        outline_px: 2,
    };

    pub(crate) fn validate(&self) -> Result<()> {
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        let colors = [
            self.background,
            self.edge_color,
            self.selection_color,
            self.hover_color,
        ];
        let valid = matches!(self.sample_count, 1 | 4)
            && unit(self.ambient)
            && unit(self.xray_opacity)
            && unit(self.tint)
            && self.outline_px <= MAX_OUTLINE_PX
            && self.plot_scale > 0.0
            && self.plot_scale.is_finite()
            && colors
                .iter()
                .all(|c| [c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite()));
        if valid {
            Ok(())
        } else {
            Err(Error::new(codes::SETTINGS_INVALID))
        }
    }
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Per-frame inputs beyond the scene and the settings. None of them writes a scene buffer.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameExtras<'a> {
    /// The object under the pointer, highlighted in the hover color over any selection.
    pub hovered: Option<ObjectId>,
    /// The selected objects, highlighted in the selection color. Unknown identifiers are
    /// ignored.
    pub selected: &'a [ObjectId],
    /// The overlay, drawn above everything.
    pub overlay: Option<&'a Overlay>,
}

/// The region of an identifier pass, in pixels of the image (x right, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickRegion {
    /// The left column.
    pub x: u32,
    /// The top row.
    pub y: u32,
    /// The width; at least 1.
    pub width: u32,
    /// The height; at least 1.
    pub height: u32,
    /// Lines are widened by this many pixels on each side, so a line within this distance of
    /// a pixel center covers it.
    pub tolerance_px: f32,
}

impl PickRegion {
    /// The single pixel `(x, y)`.
    #[must_use]
    pub fn pixel(x: u32, y: u32, tolerance_px: f32) -> Self {
        Self {
            x,
            y,
            width: 1,
            height: 1,
            tolerance_px,
        }
    }
}

/// What the identifier pass found: the item, and the element of it (the instance index of a
/// mesh item or the segment index of a line item, in path order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PickHit {
    /// The item: object identifier and sub-item index.
    pub key: ItemKey,
    /// The instance or segment within the item.
    pub element: u32,
}

/// A rendered image: rows top to bottom, four bytes per pixel, sRGB-encoded color with
/// straight alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba8: Vec<u8>,
}

impl Image {
    /// The pixel at `(x, y)`, or `None` outside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = usize::try_from((u64::from(y) * u64::from(self.width) + u64::from(x)) * 4).ok()?;
        let bytes = self.rgba8.get(at..at + 4)?;
        bytes.try_into().ok()
    }
}

/// A render pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Mesh,
    MeshDepth,
    MeshXray,
    MeshTint,
    MeshMask,
    MeshPick,
    Edge,
    EdgeTint,
    Line,
    LineTint,
    LineMask,
    LinePick,
    OverlayLine,
    OverlayFill,
    Outline,
}

#[derive(Debug)]
struct Shared {
    view_layout: wgpu::BindGroupLayout,
    outline_layout: wgpu::BindGroupLayout,
    view_pipeline: wgpu::PipelineLayout,
    outline_pipeline: wgpu::PipelineLayout,
    mesh: wgpu::ShaderModule,
    line: wgpu::ShaderModule,
    overlay: wgpu::ShaderModule,
    outline: wgpu::ShaderModule,
}

/// Pipelines and per-frame resources on one device.
#[derive(Debug)]
pub struct Renderer {
    gpu: Gpu,
    shared: Option<Shared>,
    /// The sample count and culling the cached pipelines were built for.
    key: (u32, bool),
    pipelines: HashMap<Kind, wgpu::RenderPipeline>,
}

fn attributes(offset_location: u32, formats: &[wgpu::VertexFormat]) -> Vec<wgpu::VertexAttribute> {
    let mut offset = 0;
    formats
        .iter()
        .zip(offset_location..)
        .map(|(format, location)| {
            let attribute = wgpu::VertexAttribute {
                format: *format,
                offset,
                shader_location: location,
            };
            offset += format.size();
            attribute
        })
        .collect()
}

fn narrow(value: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Narrowing to the GPU's precision is the intent.
    let narrowed = value as f32;
    narrowed
}

fn color4(c: Color) -> [f32; 4] {
    [c.r, c.g, c.b, c.a]
}

/// The view uniform shared by all scene shaders (160 bytes).
#[derive(Debug, Clone, Copy)]
struct ViewUniform {
    view_proj: [f32; 16],
    light: [f32; 4],
    viewport: [f32; 4],
    shading: [f32; 4],
    edge_color: [f32; 4],
    highlight: [f32; 4],
    codes: [u32; 4],
}

impl ViewUniform {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(160);
        for v in self
            .view_proj
            .iter()
            .chain(&self.light)
            .chain(&self.viewport)
            .chain(&self.shading)
            .chain(&self.edge_color)
            .chain(&self.highlight)
        {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.codes {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// This uniform for a highlight draw: tint color and mask code.
    fn highlighted(mut self, color: Color, strength: f32, code: u32) -> Self {
        self.highlight = [color.r, color.g, color.b, strength];
        self.codes[0] = code;
        self
    }
}

/// Everything a color frame needs besides the scene.
struct ColorFrame<'a> {
    width: u32,
    height: u32,
    samples: u32,
    settings: &'a RenderSettings,
    base: ViewUniform,
    selected: Vec<ObjectId>,
    hovered: Option<ObjectId>,
    overlay: Option<Resolved>,
}

impl Renderer {
    /// Creates a renderer on `gpu`. Pipelines are built on first use.
    #[must_use]
    pub fn new(gpu: &Gpu) -> Self {
        Self {
            gpu: gpu.clone(),
            shared: None,
            key: (0, false),
            pipelines: HashMap::new(),
        }
    }

    /// Builds the pipelines in `kinds` that are not cached for `samples` and `cull`. Runs
    /// inside the caller's error scope.
    fn ensure(&mut self, samples: u32, cull: bool, kinds: &[Kind]) {
        let device = self.gpu.device();
        if self.key != (samples, cull) {
            self.pipelines.clear();
            self.key = (samples, cull);
        }
        let shared = self.shared.get_or_insert_with(|| build_shared(device));
        for kind in kinds {
            if !self.pipelines.contains_key(kind) {
                let pipeline = build_pipeline(device, shared, *kind, samples, cull);
                self.pipelines.insert(*kind, pipeline);
            }
        }
    }

    fn pipeline(&self, kind: Kind) -> Result<&wgpu::RenderPipeline> {
        self.pipelines
            .get(&kind)
            .ok_or_else(|| Error::new(codes::VALIDATION))
    }

    fn view_bind_group(&self, uniform: &ViewUniform) -> Result<wgpu::BindGroup> {
        let shared = self
            .shared
            .as_ref()
            .ok_or_else(|| Error::new(codes::VALIDATION))?;
        let bytes = uniform.bytes();
        let device = self.gpu.device();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arc-gpu view"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue().write_buffer(&buffer, 0, &bytes);
        Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("arc-gpu view"),
            layout: &shared.view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        }))
    }

    /// Validates the settings and the image size and returns the sample count to use.
    fn check(
        &self,
        scene: &Scene,
        width: u32,
        height: u32,
        settings: &RenderSettings,
    ) -> Result<u32> {
        settings.validate()?;
        let max_side = scene
            .limits()
            .max_image_side
            .min(self.gpu.device().limits().max_texture_dimension_2d);
        if width == 0 || height == 0 || width > max_side || height > max_side {
            return Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_image_side"));
        }
        Ok(settings.sample_count.min(self.gpu.report().max_samples))
    }

    /// The camera translation origin and the view uniform of `frame` for an image of
    /// `width × height`, remapped by `window`, with a viewport of `viewport` pixels.
    #[allow(clippy::too_many_arguments)]
    fn view_uniform(
        scene: &Scene,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        settings: &RenderSettings,
        window: &Matrix4,
        viewport: (u32, u32),
    ) -> Result<(arc_base::math::Point3, ViewUniform)> {
        let aspect = f64::from(width) / f64::from(height);
        let bounds = scene
            .items()
            .map(|item| sphere_depth(frame, &item.origin, item.radius))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        let (eye, depth) = depth_origin(frame, bounds);
        let camera = camera_data(frame, aspect, depth, window)?;
        let paper = narrow(settings.plot_scale * camera.y_scale * f64::from(height) * 0.5);
        let smoothing = if settings.line_smoothing { 1.0 } else { 0.0 };
        let mode = settings.mode;
        #[allow(clippy::cast_precision_loss)] // Sides are at most the texture limit.
        let uniform = ViewUniform {
            view_proj: camera.view_proj,
            light: [
                camera.light[0],
                camera.light[1],
                camera.light[2],
                settings.ambient,
            ],
            viewport: [viewport.0 as f32, viewport.1 as f32, paper, smoothing],
            shading: [
                if mode == ShadingMode::ConsistentColors {
                    0.0
                } else {
                    1.0
                },
                if mode == ShadingMode::XRay {
                    settings.xray_opacity
                } else {
                    1.0
                },
                if mode == ShadingMode::ShadedWithEdges {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ],
            edge_color: color4(settings.edge_color),
            highlight: [0.0; 4],
            codes: [0; 4],
        };
        Ok((eye, uniform))
    }

    /// Renders `scene` seen through `frame` into an image of `width × height` pixels in the
    /// settings' shading mode, without highlights or overlay. The result is deterministic for
    /// a given adapter.
    ///
    /// # Errors
    ///
    /// As [`Renderer::render_with`].
    pub fn render_to_image(
        &mut self,
        scene: &mut Scene,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        settings: &RenderSettings,
    ) -> Result<Image> {
        self.render_with(
            scene,
            frame,
            width,
            height,
            settings,
            &FrameExtras::default(),
        )
    }

    /// Renders `scene` seen through `frame` with the highlights and overlay of `extras`.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` for invalid settings; `gpu.limit.exceeded` for a zero size or a
    /// side above the scene limit or the device's texture limit, or an overlay beyond the
    /// limits; overlay errors (see [`Overlay`]); `gpu.precision.insufficient` when the view
    /// cannot be represented in `f32`; `gpu.timeout`, `gpu.readback.failed`,
    /// `gpu.device.lost`, `gpu.out_of_memory` and `gpu.validation` from the device.
    pub fn render_with(
        &mut self,
        scene: &mut Scene,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        settings: &RenderSettings,
        extras: &FrameExtras<'_>,
    ) -> Result<Image> {
        let (_, rgba8) = self.draw(scene, frame, (width, height), settings, extras, true)?;
        Ok(Image {
            width,
            height,
            rgba8: rgba8.unwrap_or_default(),
        })
    }

    /// Renders a frame into a new color texture that can be sampled and copied, and reads it
    /// back when `readback` is set. Without readback the call returns once the device has
    /// finished the frame, so at most one frame is in flight.
    pub(crate) fn draw(
        &mut self,
        scene: &mut Scene,
        frame: &ViewFrame,
        (width, height): (u32, u32),
        settings: &RenderSettings,
        extras: &FrameExtras<'_>,
        readback: bool,
    ) -> Result<(wgpu::Texture, Option<Vec<u8>>)> {
        let samples = self.check(scene, width, height, settings)?;
        let (eye, base) = Self::view_uniform(
            scene,
            frame,
            width,
            height,
            settings,
            &Matrix4::identity(),
            (width, height),
        )?;
        let overlay = extras
            .overlay
            .map(|o| o.resolve(frame, width, height, scene.limits()))
            .transpose()?;
        let mut selected: Vec<ObjectId> = extras
            .selected
            .iter()
            .filter(|id| !scene.object_items(id).is_empty())
            .copied()
            .collect();
        selected.sort_unstable();
        selected.dedup();
        let plan = ColorFrame {
            width,
            height,
            samples,
            settings,
            base,
            selected,
            hovered: extras
                .hovered
                .filter(|id| !scene.object_items(id).is_empty()),
            overlay,
        };
        let gpu = self.gpu.clone();
        gpu.scoped(|| {
            scene.prepare_frame(&eye)?;
            let faces = settings.mode.faces();
            let mut kinds = vec![Kind::Line];
            kinds.extend(faces);
            if settings.mode.passes().contains(&Pass::Edges) {
                kinds.push(Kind::Edge);
            }
            if !plan.selected.is_empty() || plan.hovered.is_some() {
                let tint = if faces.is_some() {
                    Kind::MeshTint
                } else {
                    Kind::EdgeTint
                };
                kinds.extend([tint, Kind::LineTint, Kind::MeshMask, Kind::LineMask]);
                if settings.outline_px > 0 {
                    kinds.push(Kind::Outline);
                }
            }
            if plan.overlay.is_some() {
                kinds.extend([Kind::OverlayLine, Kind::OverlayFill]);
            }
            self.ensure(samples, settings.cull_back_faces, &kinds);
            let (texture, encoder) = self.encode_color(scene, &plan)?;
            if readback {
                let rgba8 = read_texture(&self.gpu, encoder, &texture, width, height)?;
                return Ok((texture, Some(rgba8)));
            }
            let submission = self.gpu.queue().submit([encoder.finish()]);
            wait(&self.gpu, submission)?;
            Ok((texture, None))
        })
    }

    #[allow(clippy::too_many_lines)] // The passes of a frame, in order.
    fn encode_color(
        &self,
        scene: &Scene,
        plan: &ColorFrame<'_>,
    ) -> Result<(wgpu::Texture, wgpu::CommandEncoder)> {
        let device = self.gpu.device();
        let settings = plan.settings;
        let (width, height) = (plan.width, plan.height);
        let base = self.view_bind_group(&plan.base)?;
        let selection = self.view_bind_group(&plan.base.highlighted(
            settings.selection_color,
            settings.tint,
            MASK_SELECTED,
        ))?;
        let hover = self.view_bind_group(&plan.base.highlighted(
            settings.hover_color,
            settings.tint,
            MASK_HOVERED,
        ))?;
        let highlighted = !plan.selected.is_empty() || plan.hovered.is_some();
        let groups: [(&[ObjectId], &wgpu::BindGroup); 2] = [
            (&plan.selected, &selection),
            (plan.hovered.as_slice(), &hover),
        ];
        let resolved_texture = texture(
            device,
            "arc-gpu color",
            (width, height),
            1,
            COLOR_FORMAT,
            wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let resolved = resolved_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let multisampled = (plan.samples > 1).then(|| {
            texture(
                device,
                "arc-gpu color (multisampled)",
                (width, height),
                plan.samples,
                COLOR_FORMAT,
                wgpu::TextureUsages::empty(),
            )
            .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let depth = texture(
            device,
            "arc-gpu depth",
            (width, height),
            plan.samples,
            DEPTH_FORMAT,
            wgpu::TextureUsages::empty(),
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("arc-gpu frame"),
        });

        // The highlight mask: whole objects, regardless of depth, hover over selection.
        let outline = if highlighted && settings.outline_px > 0 {
            let mask_texture = texture(
                device,
                "arc-gpu highlight mask",
                (width, height),
                1,
                MASK_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let mask = mask_texture.create_view(&wgpu::TextureViewDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("arc-gpu highlight mask"),
                    color_attachments: &[Some(attachment(&mask, None, wgpu::Color::TRANSPARENT))],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                for (objects, group) in groups {
                    pass.set_bind_group(0, group, &[]);
                    let items = || objects.iter().flat_map(|id| scene.object_items(id));
                    pass.set_pipeline(self.pipeline(Kind::MeshMask)?);
                    draw_meshes(&mut pass, scene, items(), false)?;
                    pass.set_pipeline(self.pipeline(Kind::LineMask)?);
                    draw_lines(&mut pass, scene, items())?;
                }
            }
            Some(self.outline_bind_group(&mask, settings)?)
        } else {
            None
        };

        let overlay = plan.overlay.as_ref().map(|o| overlay_buffers(device, o));
        {
            let (view, resolve_target) = match &multisampled {
                Some(ms) => (ms, Some(&resolved)),
                None => (&resolved, None),
            };
            let b = settings.background;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("arc-gpu frame"),
                color_attachments: &[Some(attachment(
                    view,
                    resolve_target,
                    wgpu::Color {
                        r: f64::from(b.r),
                        g: f64::from(b.g),
                        b: f64::from(b.b),
                        a: f64::from(b.a),
                    },
                ))],
                depth_stencil_attachment: Some(depth_attachment(&depth)),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &base, &[]);
            let faces = settings.mode.faces();
            for step in settings.mode.passes() {
                match step {
                    Pass::Meshes => {
                        if let Some(kind) = faces {
                            pass.set_pipeline(self.pipeline(kind)?);
                            draw_meshes(&mut pass, scene, scene.items(), false)?;
                        }
                    }
                    Pass::Edges => {
                        pass.set_pipeline(self.pipeline(Kind::Edge)?);
                        draw_meshes(&mut pass, scene, scene.items(), true)?;
                    }
                    Pass::Lines => {
                        pass.set_pipeline(self.pipeline(Kind::Line)?);
                        draw_lines(&mut pass, scene, scene.items())?;
                    }
                    Pass::Tint => {
                        if !highlighted {
                            continue;
                        }
                        for (objects, group) in groups {
                            pass.set_bind_group(0, group, &[]);
                            let items = || objects.iter().flat_map(|id| scene.object_items(id));
                            // Without faces, the edges carry the tint.
                            let (kind, edges) = if faces.is_some() {
                                (Kind::MeshTint, false)
                            } else {
                                (Kind::EdgeTint, true)
                            };
                            pass.set_pipeline(self.pipeline(kind)?);
                            draw_meshes(&mut pass, scene, items(), edges)?;
                            pass.set_pipeline(self.pipeline(Kind::LineTint)?);
                            draw_lines(&mut pass, scene, items())?;
                        }
                        pass.set_bind_group(0, &base, &[]);
                    }
                    Pass::Outline => {
                        if let Some(group) = &outline {
                            pass.set_pipeline(self.pipeline(Kind::Outline)?);
                            pass.set_bind_group(0, group, &[]);
                            pass.draw(0..3, 0..1);
                            pass.set_bind_group(0, &base, &[]);
                        }
                    }
                    Pass::Overlay => {
                        let (Some((segments, zeros, fill)), Some(o)) = (&overlay, &plan.overlay)
                        else {
                            continue;
                        };
                        if let Some(fill) = fill {
                            pass.set_pipeline(self.pipeline(Kind::OverlayFill)?);
                            pass.set_vertex_buffer(0, fill.slice(..));
                            pass.draw(0..o.fill_vertices, 0..1);
                        }
                        if let (Some(segments), Some(zeros)) = (segments, zeros) {
                            pass.set_pipeline(self.pipeline(Kind::OverlayLine)?);
                            pass.set_vertex_buffer(0, segments.slice(..));
                            pass.set_vertex_buffer(1, zeros.slice(..));
                            pass.draw(0..6, 0..o.segment_count);
                        }
                    }
                }
            }
        }
        Ok((resolved_texture, encoder))
    }

    fn outline_bind_group(
        &self,
        mask: &wgpu::TextureView,
        settings: &RenderSettings,
    ) -> Result<wgpu::BindGroup> {
        let shared = self
            .shared
            .as_ref()
            .ok_or_else(|| Error::new(codes::VALIDATION))?;
        let device = self.gpu.device();
        let mut bytes = Vec::with_capacity(48);
        for v in color4(settings.selection_color)
            .iter()
            .chain(&color4(settings.hover_color))
        {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let radius = i32::try_from(settings.outline_px).unwrap_or(0);
        for v in [radius, 0, 0, 0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arc-gpu outline"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue().write_buffer(&buffer, 0, &bytes);
        Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("arc-gpu outline"),
            layout: &shared.outline_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(mask),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buffer.as_entire_binding(),
                },
            ],
        }))
    }

    /// Runs the identifier pass for `region` of an image of `width × height` pixels and
    /// returns the distinct hits in it, sorted. Only the region is rendered, into a target of
    /// its size at the same pixel scale. Faces are picked in every shading mode; meshes are
    /// drawn first with depth writes, then lines (widened by the region's tolerance) where
    /// they are not behind a face, so a visible line wins over the face it lies on. A pixel
    /// shows the nearest face or, among lines, the last drawn one. A pick counts as a frame for
    /// write accounting.
    ///
    /// # Errors
    ///
    /// `gpu.pick.invalid` for an empty region, a region outside the image or a tolerance that
    /// is negative, not finite or above `max_line_width_px`; `gpu.limit.exceeded` when an
    /// instance pool is too large for identifier values; otherwise as
    /// [`Renderer::render_with`].
    pub fn pick(
        &mut self,
        scene: &mut Scene,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        settings: &RenderSettings,
        region: &PickRegion,
    ) -> Result<Vec<PickHit>> {
        let samples = self.check(scene, width, height, settings)?;
        let inside = |start: u32, size: u32, side: u32| {
            size > 0 && start.checked_add(size).is_some_and(|end| end <= side)
        };
        let tolerance = region.tolerance_px;
        if !(inside(region.x, region.width, width)
            && inside(region.y, region.height, height)
            && tolerance.is_finite()
            && (0.0..=scene.limits().max_line_width_px).contains(&tolerance))
        {
            return Err(Error::new(codes::PICK_INVALID));
        }
        let memory = scene.memory();
        let units = (memory.mesh_instance.capacity_bytes / MESH_INSTANCE_STRIDE)
            .max(memory.line_segment.capacity_bytes / SEGMENT_STRIDE);
        if units >= u64::from(LINE_BIT - 1) {
            return Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_pool_bytes"));
        }
        let window = region_window(
            (width, height),
            (region.x, region.y, region.width, region.height),
        );
        let (eye, mut uniform) = Self::view_uniform(
            scene,
            frame,
            width,
            height,
            settings,
            &window,
            (region.width, region.height),
        )?;
        uniform.shading[3] = tolerance;
        let gpu = self.gpu.clone();
        let values = gpu.scoped(|| {
            scene.prepare_frame(&eye)?;
            self.ensure(
                samples,
                settings.cull_back_faces,
                &[Kind::MeshPick, Kind::LinePick],
            );
            self.encode_pick(scene, &uniform, region)
        })?;
        let mut found: Vec<u32> = values
            .chunks_exact(4)
            .filter_map(|b| b.try_into().ok().map(u32::from_le_bytes))
            .filter(|v| *v != 0)
            .collect();
        found.sort_unstable();
        found.dedup();
        let table = scene.instance_table();
        let mut hits: Vec<PickHit> = found
            .into_iter()
            .filter_map(|v| {
                let line = v & LINE_BIT != 0;
                let index = u64::from((v & !LINE_BIT).checked_sub(1)?);
                let at = table.partition_point(|(l, range, _)| (*l, range.start) <= (line, index));
                let (l, range, key) = table.get(at.checked_sub(1)?)?;
                (*l == line && range.contains(&index)).then(|| PickHit {
                    key: *key,
                    element: u32::try_from(index - range.start).unwrap_or(u32::MAX),
                })
            })
            .collect();
        hits.sort_unstable();
        hits.dedup();
        Ok(hits)
    }

    fn encode_pick(
        &self,
        scene: &Scene,
        uniform: &ViewUniform,
        region: &PickRegion,
    ) -> Result<Vec<u8>> {
        let device = self.gpu.device();
        let size = (region.width, region.height);
        let target = texture(
            device,
            "arc-gpu identifiers",
            size,
            1,
            PICK_FORMAT,
            wgpu::TextureUsages::COPY_SRC,
        );
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = texture(
            device,
            "arc-gpu identifier depth",
            size,
            1,
            DEPTH_FORMAT,
            wgpu::TextureUsages::empty(),
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let group = self.view_bind_group(uniform)?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("arc-gpu identifiers"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("arc-gpu identifiers"),
                color_attachments: &[Some(attachment(&view, None, wgpu::Color::TRANSPARENT))],
                depth_stencil_attachment: Some(depth_attachment(&depth)),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &group, &[]);
            pass.set_pipeline(self.pipeline(Kind::MeshPick)?);
            draw_meshes(&mut pass, scene, scene.items(), false)?;
            pass.set_pipeline(self.pipeline(Kind::LinePick)?);
            draw_lines(&mut pass, scene, scene.items())?;
        }
        read_texture(&self.gpu, encoder, &target, region.width, region.height)
    }
}

/// The segment buffer with its zero translations, and the fill vertex buffer, of a resolved
/// overlay; `None` for an empty part.
type OverlayBuffers = (
    Option<wgpu::Buffer>,
    Option<wgpu::Buffer>,
    Option<wgpu::Buffer>,
);

fn overlay_buffers(device: &wgpu::Device, o: &Resolved) -> OverlayBuffers {
    use wgpu::util::DeviceExt;
    let init = |label, contents: &[u8]| {
        (!contents.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::VERTEX,
            })
        })
    };
    // Overlay segments are in pixels; their translations are zero (buffers start zeroed).
    let zeros = (o.segment_count > 0).then(|| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arc-gpu overlay translations"),
            size: u64::from(o.segment_count) * TRANSLATION_STRIDE,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        })
    });
    (
        init("arc-gpu overlay segments", &o.segments),
        zeros,
        init("arc-gpu overlay fills", &o.fill),
    )
}

fn attachment<'a>(
    view: &'a wgpu::TextureView,
    resolve_target: Option<&'a wgpu::TextureView>,
    clear: wgpu::Color,
) -> wgpu::RenderPassColorAttachment<'a> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(clear),
            store: wgpu::StoreOp::Store,
        },
    }
}

fn depth_attachment(view: &wgpu::TextureView) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        // Reversed-Z: clear to the far value.
        depth_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Clear(0.0),
            store: wgpu::StoreOp::Discard,
        }),
        stencil_ops: None,
    }
}

fn to_u32(range: std::ops::Range<u64>) -> Result<std::ops::Range<u32>> {
    match (u32::try_from(range.start), u32::try_from(range.end)) {
        (Ok(start), Ok(end)) => Ok(start..end),
        _ => Err(Error::new(codes::LIMIT_EXCEEDED)),
    }
}

/// Draws the faces, or with `edges` the feature edges, of the mesh items in `items`. The
/// pipeline and bind group are set by the caller.
fn draw_meshes<'a>(
    pass: &mut wgpu::RenderPass<'_>,
    scene: &Scene,
    items: impl Iterator<Item = &'a ItemEntry>,
    edges: bool,
) -> Result<()> {
    let buffers = scene.buffers();
    let Some(translations) = buffers.mesh_translations else {
        return Ok(());
    };
    pass.set_vertex_buffer(0, buffers.vertices.slice(..));
    pass.set_vertex_buffer(1, buffers.mesh_instances.slice(..));
    pass.set_vertex_buffer(2, translations.slice(..));
    pass.set_index_buffer(buffers.indices.slice(..), wgpu::IndexFormat::Uint32);
    for item in items {
        let ItemGpu::Mesh { geometry, .. } = item.gpu else {
            continue;
        };
        let (Some((first_vertex, first_index, count)), Some(instances)) = (
            scene.geometry_ranges(geometry),
            scene.instance_range(item.gpu),
        ) else {
            continue;
        };
        let (first_index, count) = if edges {
            match scene.edge_range(geometry) {
                Some(range) => range,
                None => continue,
            }
        } else {
            (first_index, count)
        };
        let indices = to_u32(first_index..first_index + u64::from(count))?;
        let base = i32::try_from(first_vertex).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
        pass.draw_indexed(indices, base, to_u32(instances)?);
    }
    Ok(())
}

/// Draws the line items in `items`. The pipeline and bind group are set by the caller.
fn draw_lines<'a>(
    pass: &mut wgpu::RenderPass<'_>,
    scene: &Scene,
    items: impl Iterator<Item = &'a ItemEntry>,
) -> Result<()> {
    let buffers = scene.buffers();
    let Some(translations) = buffers.segment_translations else {
        return Ok(());
    };
    pass.set_vertex_buffer(0, buffers.segments.slice(..));
    pass.set_vertex_buffer(1, translations.slice(..));
    for item in items {
        if !matches!(item.gpu, ItemGpu::Lines { .. }) {
            continue;
        }
        let Some(range) = scene.instance_range(item.gpu) else {
            continue;
        };
        pass.draw(0..6, to_u32(range)?);
    }
    Ok(())
}

fn build_shared(device: &wgpu::Device) -> Shared {
    let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("arc-gpu view"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let outline_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("arc-gpu outline"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = |label, layout: &wgpu::BindGroupLayout| {
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        })
    };
    let module = |label, source: &'static str| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        })
    };
    Shared {
        view_pipeline: pipeline_layout("arc-gpu view", &view_layout),
        outline_pipeline: pipeline_layout("arc-gpu outline", &outline_layout),
        view_layout,
        outline_layout,
        mesh: module("arc-gpu meshes", include_str!("shaders/mesh.wgsl")),
        line: module("arc-gpu lines", include_str!("shaders/line.wgsl")),
        overlay: module("arc-gpu overlay", include_str!("shaders/overlay.wgsl")),
        outline: module("arc-gpu outline", include_str!("shaders/outline.wgsl")),
    }
}

/// The color target of a pipeline.
enum Target {
    /// The frame's color target, with a blend state and write mask.
    Color(Option<wgpu::BlendState>, wgpu::ColorWrites),
    /// A single-sample integer target (mask or identifiers).
    Integer(wgpu::TextureFormat),
}

/// The vertex input of a pipeline.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    Mesh,
    Segment,
    Fill,
    None,
}

#[allow(clippy::too_many_lines)] // One table row per pipeline.
fn build_pipeline(
    device: &wgpu::Device,
    shared: &Shared,
    kind: Kind,
    samples: u32,
    cull: bool,
) -> wgpu::RenderPipeline {
    use wgpu::CompareFunction::{Always, Greater, GreaterEqual};
    use wgpu::PrimitiveTopology::{LineList, TriangleList};
    let alpha = Target::Color(
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    let opaque = Target::Color(None, wgpu::ColorWrites::ALL);
    let mask = Target::Integer(MASK_FORMAT);
    let ids = Target::Integer(PICK_FORMAT);
    // (module, vertex entry, fragment entry, input, topology, culled, depth (write, compare)
    // or none, target).
    let (module, vs, fs, input, topology, culled, depth, target) = match kind {
        Kind::Mesh => (
            &shared.mesh,
            "vs_main",
            "fs_main",
            Input::Mesh,
            TriangleList,
            true,
            Some((true, Greater)),
            opaque,
        ),
        Kind::MeshDepth => (
            &shared.mesh,
            "vs_main",
            "fs_main",
            Input::Mesh,
            TriangleList,
            true,
            Some((true, Greater)),
            Target::Color(None, wgpu::ColorWrites::empty()),
        ),
        Kind::MeshXray => (
            &shared.mesh,
            "vs_main",
            "fs_main",
            Input::Mesh,
            TriangleList,
            true,
            Some((false, Always)),
            alpha,
        ),
        Kind::MeshTint => (
            &shared.mesh,
            "vs_main",
            "fs_tint",
            Input::Mesh,
            TriangleList,
            true,
            Some((false, GreaterEqual)),
            alpha,
        ),
        Kind::MeshMask => (
            &shared.mesh,
            "vs_main",
            "fs_mask",
            Input::Mesh,
            TriangleList,
            false,
            None,
            mask,
        ),
        Kind::MeshPick => (
            &shared.mesh,
            "vs_main",
            "fs_pick",
            Input::Mesh,
            TriangleList,
            true,
            Some((true, Greater)),
            ids,
        ),
        Kind::Edge => (
            &shared.mesh,
            "vs_edge",
            "fs_edge",
            Input::Mesh,
            LineList,
            false,
            Some((false, GreaterEqual)),
            alpha,
        ),
        Kind::EdgeTint => (
            &shared.mesh,
            "vs_edge",
            "fs_tint",
            Input::Mesh,
            LineList,
            false,
            Some((false, GreaterEqual)),
            alpha,
        ),
        Kind::Line => (
            &shared.line,
            "vs_main",
            "fs_main",
            Input::Segment,
            TriangleList,
            false,
            Some((false, GreaterEqual)),
            alpha,
        ),
        Kind::LineTint => (
            &shared.line,
            "vs_main",
            "fs_tint",
            Input::Segment,
            TriangleList,
            false,
            Some((false, GreaterEqual)),
            alpha,
        ),
        Kind::LineMask => (
            &shared.line,
            "vs_main",
            "fs_mask",
            Input::Segment,
            TriangleList,
            false,
            None,
            mask,
        ),
        Kind::LinePick => (
            &shared.line,
            "vs_main",
            "fs_pick",
            Input::Segment,
            TriangleList,
            false,
            Some((false, GreaterEqual)),
            ids,
        ),
        Kind::OverlayLine => (
            &shared.line,
            "vs_main",
            "fs_main",
            Input::Segment,
            TriangleList,
            false,
            Some((false, Always)),
            alpha,
        ),
        Kind::OverlayFill => (
            &shared.overlay,
            "vs_main",
            "fs_main",
            Input::Fill,
            TriangleList,
            false,
            Some((false, Always)),
            alpha,
        ),
        Kind::Outline => (
            &shared.outline,
            "vs_main",
            "fs_main",
            Input::None,
            TriangleList,
            false,
            Some((false, Always)),
            opaque,
        ),
    };
    use wgpu::VertexFormat::{Float32x2, Float32x3, Float32x4, Uint32x2, Uint32x4};
    let translation = attributes(7, &[Float32x3]);
    let vertex = attributes(0, &[Float32x3, Float32x3]);
    let instance = attributes(2, &[Float32x4, Float32x4, Float32x4, Float32x4, Uint32x4]);
    let segment = attributes(
        0,
        &[
            Float32x3, Float32x3, Float32x3, Float32x3, Float32x4, Float32x2, Uint32x2,
        ],
    );
    let fill = attributes(0, &[Float32x2, Float32x4]);
    let layout = |stride, step_mode, attributes| {
        Some(wgpu::VertexBufferLayout {
            array_stride: stride,
            step_mode,
            attributes,
        })
    };
    use wgpu::VertexStepMode::{Instance as PerInstance, Vertex as PerVertex};
    let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = match input {
        Input::Mesh => vec![
            layout(VERTEX_STRIDE, PerVertex, &vertex),
            layout(MESH_INSTANCE_STRIDE, PerInstance, &instance),
            layout(TRANSLATION_STRIDE, PerInstance, &translation),
        ],
        Input::Segment => vec![
            layout(SEGMENT_STRIDE, PerInstance, &segment),
            layout(TRANSLATION_STRIDE, PerInstance, &translation),
        ],
        Input::Fill => vec![layout(FILL_VERTEX_STRIDE, PerVertex, &fill)],
        Input::None => vec![],
    };
    let (format, blend, write_mask, count) = match target {
        Target::Color(blend, mask) => (COLOR_FORMAT, blend, mask, samples),
        Target::Integer(format) => (format, None, wgpu::ColorWrites::ALL, 1),
    };
    let layout = if kind == Kind::Outline {
        &shared.outline_pipeline
    } else {
        &shared.view_pipeline
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("arc-gpu pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: (culled && cull).then_some(wgpu::Face::Back),
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: depth.map(|(write, compare)| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            // Reversed-Z: nearer is greater.
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count,
            ..wgpu::MultisampleState::default()
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    samples: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: usage | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// Submits `encoder` with a copy of `texture` (four bytes per texel) and reads it back
/// without the row padding of the copy.
/// Waits until the device has finished `submission`, at most [`READBACK_TIMEOUT`].
fn wait(gpu: &Gpu, submission: wgpu::SubmissionIndex) -> Result<()> {
    match gpu.device().poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(READBACK_TIMEOUT),
    }) {
        Ok(_) => Ok(()),
        Err(wgpu::PollError::Timeout) => Err(Error::new(codes::TIMEOUT)),
        Err(other) => {
            gpu.check()?;
            Err(Error::new(codes::VALIDATION).with_source(other))
        }
    }
}

fn read_texture(
    gpu: &Gpu,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let device = gpu.device();
    // Rows of the copy are padded to the copy alignment.
    let row = u64::from(width) * 4;
    let align = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let padded = row.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("arc-gpu readback"),
        size: padded * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let padded_u32 = u32::try_from(padded).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_u32),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let submission = gpu.queue().submit([encoder.finish()]);
    let mapped: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    let signal = Arc::clone(&mapped);
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        *signal.lock().unwrap_or_else(PoisonError::into_inner) = Some(result.is_ok());
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(READBACK_TIMEOUT),
        })
        .map_err(|e| match e {
            wgpu::PollError::Timeout => Error::new(codes::TIMEOUT),
            other => Error::new(codes::READBACK_FAILED).with_source(other),
        })?;
    if *mapped.lock().unwrap_or_else(PoisonError::into_inner) != Some(true) {
        gpu.check()?;
        return Err(Error::new(codes::READBACK_FAILED));
    }
    let view = readback
        .get_mapped_range(..)
        .map_err(|e| Error::new(codes::READBACK_FAILED).with_source(e))?;
    let row_len = usize::try_from(row).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
    let padded_len = usize::try_from(padded).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
    let mut out = Vec::with_capacity(row_len * usize::try_from(height).unwrap_or(0));
    for line in view.chunks(padded_len) {
        out.extend_from_slice(
            line.get(..row_len)
                .ok_or_else(|| Error::new(codes::READBACK_FAILED))?,
        );
    }
    drop(view);
    readback.unmap();
    Ok(out)
}
