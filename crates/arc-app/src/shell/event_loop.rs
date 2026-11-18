//! The `winit` event loop of the windowed shell: one main window presenting
//! [`Shell::frame`] through `egui-winit` and `egui-wgpu` on the shared [`Gpu`] device, with
//! viewport frames registered as textures, native menus and dialogs, and the window placement
//! restored from and saved to the window-state file.
//!
//! This module is compiled and linted on every platform but never runs in the tests: it needs
//! a display. Everything it calls besides `winit`, the surface and the egui renderer is tested
//! headless.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use arc_base::{Error, Result};
use arc_gpu::{Gpu, wgpu};
use arc_ui::egui;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use super::platform::{self, Done, FileDialogs, NativeDialogs, NativeMenu};
use super::{FrameInput, Request, Shell, ShellConfig};
use crate::application::{Application, TabId};
use crate::codes;
use crate::locale::Localizer;
use crate::window_state::{Monitor, Placement, WindowStates, rescale};

/// The identifier of the main window in the window-state file.
pub const MAIN_WINDOW: &str = "main";

/// What the event loop starts with.
pub struct Startup {
    /// The application, with its settings and recent files loaded.
    pub app: Application,
    /// The shell configuration of the product.
    pub config: ShellConfig,
    /// The message source.
    pub localizer: Localizer,
    /// The device viewports and the window render on.
    pub gpu: Gpu,
    /// The window title.
    pub title: String,
    /// The window-state file; `None` keeps placements in memory only.
    pub window_states: Option<PathBuf>,
}

impl std::fmt::Debug for Startup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Startup")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

fn window_error(e: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::new(codes::WINDOW_FAILED).with_source(e)
}

/// Runs the shell until the user quits.
///
/// # Errors
/// `app.window.failed` when the event loop or the window cannot be created;
/// `app.window.surface_failed` when the device cannot present to the window.
pub fn run(startup: Startup) -> Result<()> {
    let event_loop = EventLoop::new().map_err(window_error)?;
    let (states, _) = match &startup.window_states {
        Some(path) => WindowStates::load(path)?,
        None => (WindowStates::new(), None),
    };
    let shell = Shell::new(&startup.app, startup.config, Some(startup.gpu.clone()));
    let mut handler = Handler {
        app: startup.app,
        shell,
        localizer: startup.localizer,
        gpu: startup.gpu,
        title: startup.title,
        states,
        states_path: startup.window_states,
        menu: platform::native_menu(),
        native_menu: false,
        window: None,
        failure: None,
        started: Instant::now(),
    };
    event_loop.run_app(&mut handler).map_err(window_error)?;
    handler.failure.map_or(Ok(()), Err)
}

/// One viewport texture copied into the format the egui renderer samples.
struct Registered {
    number: u64,
    texture: wgpu::Texture,
    id: egui::TextureId,
}

struct Open {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    textures: BTreeMap<TabId, Registered>,
    /// The scale factor before the latest change.
    scale: f64,
}

struct Handler {
    app: Application,
    shell: Shell,
    localizer: Localizer,
    gpu: Gpu,
    title: String,
    states: WindowStates,
    states_path: Option<PathBuf>,
    menu: Box<dyn NativeMenu>,
    native_menu: bool,
    window: Option<Open>,
    failure: Option<Error>,
    started: Instant,
}

fn monitors(event_loop: &ActiveEventLoop) -> (Vec<Monitor>, usize) {
    let primary = event_loop.primary_monitor();
    let list: Vec<_> = event_loop.available_monitors().collect();
    let at = primary
        .and_then(|p| list.iter().position(|m| *m == p))
        .unwrap_or(0);
    let monitors = list
        .iter()
        .map(|m| Monitor {
            position: [m.position().x, m.position().y],
            size: [m.size().width, m.size().height],
            scale: m.scale_factor(),
        })
        .collect();
    (monitors, at)
}

impl Handler {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Error) {
        self.failure.get_or_insert(error);
        event_loop.exit();
    }

    fn open_window(&mut self, event_loop: &ActiveEventLoop) -> Result<Open> {
        let (list, primary) = monitors(event_loop);
        let mut attributes = Window::default_attributes().with_title(self.title.clone());
        if let Some(p) = self.states.restore(&list, primary, MAIN_WINDOW) {
            attributes = attributes
                .with_position(PhysicalPosition::new(p.position[0], p.position[1]))
                .with_inner_size(PhysicalSize::new(p.size[0], p.size[1]))
                .with_maximized(p.maximized);
        }
        let window = Arc::new(event_loop.create_window(attributes).map_err(window_error)?);
        let surface_error =
            |e: wgpu::CreateSurfaceError| Error::new(codes::WINDOW_SURFACE_FAILED).with_source(e);
        let surface = self
            .gpu
            .instance()
            .create_surface(Arc::clone(&window))
            .map_err(surface_error)?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(self.gpu.adapter());
        // egui writes gamma-space colors, so a non-sRGB format shows them unchanged.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| Error::new(codes::WINDOW_SURFACE_FAILED))?;
        let mut config = surface
            .get_default_config(self.gpu.adapter(), size.width.max(1), size.height.max(1))
            .ok_or_else(|| Error::new(codes::WINDOW_SURFACE_FAILED))?;
        config.format = format;
        surface.configure(self.gpu.device(), &config);
        let ctx = egui::Context::default();
        if let Some((_, font)) = platform::system_font() {
            platform::install_font(&ctx, font);
        }
        #[allow(clippy::cast_possible_truncation, reason = "a scale factor")]
        let scale = window.scale_factor() as f32;
        let max_side = usize::try_from(self.gpu.device().limits().max_texture_dimension_2d).ok();
        let egui = egui_winit::State::new(
            ctx,
            egui::ViewportId::ROOT,
            &window,
            Some(scale),
            None,
            max_side,
        );
        let renderer = egui_wgpu::Renderer::new(
            self.gpu.device(),
            format,
            egui_wgpu::RendererOptions::default(),
        );
        let entries = self.shell.menu_entries(&self.app, &self.localizer);
        self.native_menu = self.menu.install(&entries);
        let scale = window.scale_factor();
        Ok(Open {
            window,
            surface,
            config,
            egui,
            renderer,
            textures: BTreeMap::new(),
            scale,
        })
    }

    fn save_placement(&mut self, event_loop: &ActiveEventLoop) {
        let Some(w) = &self.window else {
            return;
        };
        let (list, _) = monitors(event_loop);
        let position = w.window.outer_position().unwrap_or_default();
        let size = w.window.inner_size();
        self.states.save(
            &list,
            MAIN_WINDOW,
            Placement {
                position: [position.x, position.y],
                size: [size.width, size.height],
                maximized: w.window.is_maximized(),
            },
        );
    }

    fn write_states(&self) {
        if let Some(path) = &self.states_path {
            // A placement that cannot be written is not worth stopping the shutdown for.
            let _ = self.states.save_to(path);
        }
    }

    /// Copies new viewport frames into textures the egui renderer samples.
    fn register_frames(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(w) = self.window.as_mut() else {
            return;
        };
        let device = self.gpu.device();
        let frames = self.shell.viewport_frames();
        w.textures
            .retain(|tab, _| frames.iter().any(|(t, _)| t == tab));
        for (tab, frame) in frames {
            let size = wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            };
            let current = w.textures.get(&tab);
            if current.is_some_and(|r| r.number == frame.number) {
                continue;
            }
            let reuse = current
                .filter(|r| r.texture.size() == size)
                .map(|r| r.texture.clone());
            let texture = reuse.unwrap_or_else(|| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("arc-app viewport"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    // The egui renderer samples gamma-space texels; the frame is sRGB, and a
                    // copy between the two formats keeps the bytes.
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            });
            encoder.copy_texture_to_texture(
                frame.texture.as_image_copy(),
                texture.as_image_copy(),
                size,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let id = match w.textures.get(&tab) {
                Some(r) => {
                    w.renderer.update_egui_texture_from_wgpu_texture(
                        device,
                        &view,
                        wgpu::FilterMode::Linear,
                        r.id,
                    );
                    r.id
                }
                None => w
                    .renderer
                    .register_native_texture(device, &view, wgpu::FilterMode::Linear),
            };
            w.textures.insert(
                tab,
                Registered {
                    number: frame.number,
                    texture,
                    id,
                },
            );
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let device = self.gpu.device().clone();
        let queue = self.gpu.queue().clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("arc-app frame"),
        });
        self.register_frames(&mut encoder);
        let Some(w) = self.window.as_mut() else {
            return;
        };
        let raw = w.egui.take_egui_input(&w.window);
        let ctx = w.egui.egui_ctx().clone();
        let textures: BTreeMap<TabId, egui::TextureId> =
            w.textures.iter().map(|(t, r)| (*t, r.id)).collect();
        let input = FrameInput {
            raw,
            time: self.started.elapsed().as_secs_f64(),
            localize: &self.localizer,
            textures: &textures,
            menu_intents: self.menu.poll(),
            native_menu: self.native_menu,
        };
        let out = self.shell.frame(&mut self.app, &ctx, input);
        let mut full = out.full;
        w.egui
            .handle_platform_output(&w.window, full.platform_output);
        let jobs = ctx.tessellate(full.shapes, full.pixels_per_point);
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas {
                w.renderer.update_texture(&device, &queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w.config.width, w.config.height],
            pixels_per_point: full.pixels_per_point,
        };
        let surface_texture = match w.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                w.surface.configure(&device, &w.config);
                None
            }
            _ => None,
        };
        let mut buffers = w
            .renderer
            .update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
        if let Some(target) = &surface_texture {
            let view = target
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("arc-app egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            w.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        buffers.push(encoder.finish());
        queue.submit(buffers);
        if let Some(target) = surface_texture {
            queue.present(target);
        }
        for id in &full.textures_delta.free {
            w.renderer.free_texture(id);
        }
        // Every delta was applied above; dropping applied deltas uncleared is a fault in egui.
        full.textures_delta.clear();
        if out.busy
            || full
                .viewport_output
                .values()
                .any(|v| v.repaint_delay.is_zero())
        {
            w.window.request_redraw();
        }
        for request in out.requests {
            self.carry_out(event_loop, &request);
        }
    }

    fn carry_out(&mut self, event_loop: &ActiveEventLoop, request: &Request) {
        let mut dialogs = NativeDialogs {
            localize: Some(&self.localizer),
        };
        let dialogs: &mut dyn FileDialogs = &mut dialogs;
        match platform::carry_out(&mut self.app, dialogs, request) {
            Ok(Done::Quit) => {
                self.save_placement(event_loop);
                self.write_states();
                event_loop.exit();
            }
            Ok(_) => {}
            Err(e) => self.shell.error(e),
        }
        if let Some(w) = &self.window {
            w.window.request_redraw();
        }
    }
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match self.open_window(event_loop) {
            Ok(w) => {
                w.window.request_redraw();
                self.window = Some(w);
            }
            Err(e) => self.fail(event_loop, e),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(w) = self.window.as_mut() else {
            return;
        };
        let response = w.egui.on_window_event(&w.window, &event);
        match event {
            WindowEvent::CloseRequested => self.carry_out(event_loop, &Request::Quit),
            WindowEvent::Resized(size) => {
                w.config.width = size.width.max(1);
                w.config.height = size.height.max(1);
                w.surface.configure(self.gpu.device(), &w.config);
                w.window.request_redraw();
                self.save_placement(event_loop);
            }
            WindowEvent::Moved(_) => self.save_placement(event_loop),
            WindowEvent::ScaleFactorChanged {
                scale_factor,
                mut inner_size_writer,
            } => {
                let size = w.window.inner_size();
                let [width, height] = rescale([size.width, size.height], w.scale, scale_factor);
                w.scale = scale_factor;
                let _ = inner_size_writer.request_inner_size(PhysicalSize::new(width, height));
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {
                if response.repaint {
                    w.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.shell.busy() {
            if let Some(w) = &self.window {
                w.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}
