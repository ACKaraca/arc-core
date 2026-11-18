//! The frame and headless rendering (CORE-GPU-007, CORE-GPU-008, CORE-GPU-031).
//!
//! A frame is a list of [`Pass`]es encoded in order into one render pass on an offscreen
//! target. [`Renderer::render_to_image`] renders a [`Scene`] seen through a [`ViewFrame`] to
//! an [`Image`] without a window, on any adapter.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use arc_base::{Color, Error, Result};
use arc_geom::view::ViewFrame;

use crate::adapter::Gpu;
use crate::camera::{camera_data, depth_origin, sphere_depth};
use crate::error::codes;
use crate::scene::{
    ItemGpu, MESH_INSTANCE_STRIDE, SEGMENT_STRIDE, Scene, TRANSLATION_STRIDE, VERTEX_STRIDE,
};

/// The color target format: 8-bit sRGB with alpha; blending happens in linear space.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The depth target format.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// How long reading an image back may take.
pub const READBACK_TIMEOUT: Duration = Duration::from_secs(60);

/// A pass of a frame. Later units add passes (picking, highlights, overlays) to this list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Opaque mesh instances with depth writes.
    Meshes,
    /// Lines, depth-tested against the meshes, blended by coverage.
    Lines,
}

/// The passes of the default frame, in order.
pub const DEFAULT_FRAME: [Pass; 2] = [Pass::Meshes, Pass::Lines];

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
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            sample_count: 1,
            cull_back_faces: false,
            line_smoothing: true,
            background: Color::BLACK,
            ambient: 0.25,
            plot_scale: 1.0,
        }
    }
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

#[derive(Debug)]
struct Pipelines {
    samples: u32,
    cull: bool,
    meshes: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

/// Pipelines and per-frame resources on one device.
#[derive(Debug)]
pub struct Renderer {
    gpu: Gpu,
    pipelines: Option<Pipelines>,
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

impl Renderer {
    /// Creates a renderer on `gpu`. Pipelines are built on first use.
    #[must_use]
    pub fn new(gpu: &Gpu) -> Self {
        Self {
            gpu: gpu.clone(),
            pipelines: None,
        }
    }

    fn pipelines(&mut self, samples: u32, cull: bool) -> Result<&Pipelines> {
        let stale = self
            .pipelines
            .as_ref()
            .is_none_or(|p| p.samples != samples || p.cull != cull);
        if stale {
            let built = self
                .gpu
                .scoped(|| Ok(build_pipelines(self.gpu.device(), samples, cull)))?;
            self.pipelines = Some(built);
        }
        self.pipelines
            .as_ref()
            .ok_or_else(|| Error::new(codes::VALIDATION))
    }

    /// Renders `scene` seen through `frame` into an image of `width × height` pixels with the
    /// [default frame](DEFAULT_FRAME). The result is deterministic for a given adapter.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` for invalid settings; `gpu.limit.exceeded` for a zero size or a
    /// side above the scene limit or the device's texture limit; `gpu.precision.insufficient` when the view cannot be
    /// represented in `f32`; `gpu.timeout`, `gpu.readback.failed`, `gpu.device.lost`,
    /// `gpu.out_of_memory` and `gpu.validation` from the device.
    pub fn render_to_image(
        &mut self,
        scene: &mut Scene,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        settings: &RenderSettings,
    ) -> Result<Image> {
        let valid = matches!(settings.sample_count, 1 | 4)
            && (0.0..=1.0).contains(&settings.ambient)
            && settings.plot_scale > 0.0
            && settings.plot_scale.is_finite();
        if !valid {
            return Err(Error::new(codes::SETTINGS_INVALID));
        }
        let max_side = scene
            .limits()
            .max_image_side
            .min(self.gpu.device().limits().max_texture_dimension_2d);
        if width == 0 || height == 0 || width > max_side || height > max_side {
            return Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_image_side"));
        }
        let samples = settings.sample_count.min(self.gpu.report().max_samples);
        let aspect = f64::from(width) / f64::from(height);
        let bounds = scene
            .items()
            .map(|item| sphere_depth(frame, &item.origin, item.radius))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        let (eye, depth) = depth_origin(frame, bounds);
        let camera = camera_data(frame, aspect, depth)?;
        let gpu = self.gpu.clone();
        gpu.scoped(|| {
            scene.prepare_frame(&eye)?;
            let pipelines = self.pipelines(samples, settings.cull_back_faces)?;
            let mut uniform = Vec::with_capacity(96);
            for v in camera.view_proj {
                uniform.extend_from_slice(&v.to_le_bytes());
            }
            for v in [
                camera.light[0],
                camera.light[1],
                camera.light[2],
                settings.ambient,
            ] {
                uniform.extend_from_slice(&v.to_le_bytes());
            }
            let paper = narrow(settings.plot_scale * camera.y_scale * f64::from(height) * 0.5);
            let smoothing = if settings.line_smoothing {
                1.0_f32
            } else {
                0.0
            };
            #[allow(clippy::cast_precision_loss)] // Sides are at most the texture limit.
            for v in [width as f32, height as f32, paper, smoothing] {
                uniform.extend_from_slice(&v.to_le_bytes());
            }
            encode_and_read(
                &gpu,
                pipelines,
                scene,
                &uniform,
                width,
                height,
                samples,
                settings.background,
            )
        })
    }
}

#[allow(clippy::too_many_lines)] // One descriptor per pipeline, written out.
fn build_pipelines(device: &wgpu::Device, samples: u32, cull: bool) -> Pipelines {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("arc-gpu view"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    use wgpu::VertexFormat::{Float32x2, Float32x3, Float32x4, Uint32x2, Uint32x4};
    let translation = attributes(7, &[Float32x3]);
    let make = |source: &str,
                label: &str,
                buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
                mesh: bool| {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let blend = if mesh {
            None
        } else {
            Some(wgpu::BlendState::ALPHA_BLENDING)
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: (mesh && cull).then_some(wgpu::Face::Back),
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(mesh),
                // Reversed-Z: nearer is greater.
                depth_compare: Some(if mesh {
                    wgpu::CompareFunction::Greater
                } else {
                    wgpu::CompareFunction::GreaterEqual
                }),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..wgpu::MultisampleState::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let vertex = attributes(0, &[Float32x3, Float32x3]);
    let instance = attributes(2, &[Float32x4, Float32x4, Float32x4, Float32x4, Uint32x4]);
    let meshes = make(
        include_str!("shaders/mesh.wgsl"),
        "arc-gpu meshes",
        &[
            Some(wgpu::VertexBufferLayout {
                array_stride: VERTEX_STRIDE,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: MESH_INSTANCE_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: TRANSLATION_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &translation,
            }),
        ],
        true,
    );
    let segment = attributes(
        0,
        &[
            Float32x3, Float32x3, Float32x3, Float32x3, Float32x4, Float32x2, Uint32x2,
        ],
    );
    let lines = make(
        include_str!("shaders/line.wgsl"),
        "arc-gpu lines",
        &[
            Some(wgpu::VertexBufferLayout {
                array_stride: SEGMENT_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &segment,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: TRANSLATION_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &translation,
            }),
        ],
        false,
    );
    Pipelines {
        samples,
        cull,
        meshes,
        lines,
        layout,
    }
}

fn target(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    samples: u32,
    format: wgpu::TextureFormat,
    copy: bool,
) -> wgpu::TextureView {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if copy {
        usage |= wgpu::TextureUsages::COPY_SRC;
    }
    device
        .create_texture(&wgpu::TextureDescriptor {
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
            usage,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn encode_and_read(
    gpu: &Gpu,
    pipelines: &Pipelines,
    scene: &Scene,
    uniform: &[u8],
    width: u32,
    height: u32,
    samples: u32,
    background: Color,
) -> Result<Image> {
    let device = gpu.device();
    let queue = gpu.queue();
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("arc-gpu view"),
        size: uniform.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buffer, 0, uniform);
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("arc-gpu view"),
        layout: &pipelines.layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    let resolved_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("arc-gpu color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let resolved = resolved_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let multisampled = (samples > 1).then(|| {
        target(
            device,
            "arc-gpu color (multisampled)",
            width,
            height,
            samples,
            COLOR_FORMAT,
            false,
        )
    });
    let depth = target(
        device,
        "arc-gpu depth",
        width,
        height,
        samples,
        DEPTH_FORMAT,
        false,
    );
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("arc-gpu frame"),
    });
    {
        let (view, resolve_target) = match &multisampled {
            Some(ms) => (ms, Some(&resolved)),
            None => (&resolved, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("arc-gpu frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(background.r),
                        g: f64::from(background.g),
                        b: f64::from(background.b),
                        a: f64::from(background.a),
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                // Reversed-Z: clear to the far value.
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &bind_group, &[]);
        let buffers = scene.buffers();
        for step in DEFAULT_FRAME {
            match step {
                Pass::Meshes => {
                    let Some(translations) = buffers.mesh_translations else {
                        continue;
                    };
                    pass.set_pipeline(&pipelines.meshes);
                    pass.set_vertex_buffer(0, buffers.vertices.slice(..));
                    pass.set_vertex_buffer(1, buffers.mesh_instances.slice(..));
                    pass.set_vertex_buffer(2, translations.slice(..));
                    pass.set_index_buffer(buffers.indices.slice(..), wgpu::IndexFormat::Uint32);
                    for item in scene.items() {
                        let ItemGpu::Mesh { geometry, .. } = item.gpu else {
                            continue;
                        };
                        let (Some((first_vertex, first_index, count)), Some(instances)) = (
                            scene.geometry_ranges(geometry),
                            scene.instance_range(item.gpu),
                        ) else {
                            continue;
                        };
                        let (Ok(base), Ok(first), Ok(start), Ok(end)) = (
                            i32::try_from(first_vertex),
                            u32::try_from(first_index),
                            u32::try_from(instances.start),
                            u32::try_from(instances.end),
                        ) else {
                            return Err(Error::new(codes::LIMIT_EXCEEDED));
                        };
                        pass.draw_indexed(first..first + count, base, start..end);
                    }
                }
                Pass::Lines => {
                    let Some(translations) = buffers.segment_translations else {
                        continue;
                    };
                    pass.set_pipeline(&pipelines.lines);
                    pass.set_vertex_buffer(0, buffers.segments.slice(..));
                    pass.set_vertex_buffer(1, translations.slice(..));
                    for item in scene.items() {
                        if !matches!(item.gpu, ItemGpu::Lines { .. }) {
                            continue;
                        }
                        let Some(range) = scene.instance_range(item.gpu) else {
                            continue;
                        };
                        let (Ok(start), Ok(end)) =
                            (u32::try_from(range.start), u32::try_from(range.end))
                        else {
                            return Err(Error::new(codes::LIMIT_EXCEEDED));
                        };
                        pass.draw(0..6, start..end);
                    }
                }
            }
        }
    }
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
            texture: &resolved_texture,
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
    let submission = queue.submit([encoder.finish()]);
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
    let mut rgba8 = Vec::with_capacity(row_len * usize::try_from(height).unwrap_or(0));
    for line in view.chunks(padded_len) {
        rgba8.extend_from_slice(
            line.get(..row_len)
                .ok_or_else(|| Error::new(codes::READBACK_FAILED))?,
        );
    }
    drop(view);
    readback.unmap();
    Ok(Image {
        width,
        height,
        rgba8,
    })
}
