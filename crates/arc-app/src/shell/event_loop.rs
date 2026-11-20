//! The `winit` event loop of the windowed shell: a main window presenting [`Shell::frame`]
//! and one secondary window per floating dock tree presenting [`Shell::frame_window`], each
//! through `egui-winit` and `egui-wgpu` on the shared [`Gpu`] device, with viewport frames
//! registered as textures, native menus and dialogs, and every window's placement restored
//! from and saved to the window-state file. At start the shell loads the user files
//! ([`Shell::start`]) and the language setting is applied; a settings change applies it again.
//!
//! This module is compiled and linted on every platform but never runs in the tests: it needs
//! a display. Everything it calls besides `winit`, the surface and the egui renderer is tested
//! headless.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_base::{Error, Result};
use arc_gpu::{Gpu, wgpu};
use arc_ui::egui;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use super::platform::{self, Done, FileDialogs, NativeDialogs, NativeMenu};
use super::{FrameInput, Request, Shell, ShellConfig, UserFiles, WindowModel, prefs};
use crate::application::{Application, TabId};
use crate::automation::{self, Automation};
use crate::codes;
use crate::locale::Localizer;
use crate::settings::SettingValue;
use crate::single_instance::SingleInstance;

/// How often a running instance looks for the files other starts forward.
const FORWARD_POLL: Duration = Duration::from_millis(250);
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
    /// The user settings and recent-files files; `None` keeps both in memory only.
    pub files: Option<UserFiles>,
    /// The operating system's locale list, for `ui.language` = `system`.
    pub os_locales: Vec<String>,
    /// The log directory: the subscriber is installed from the log settings once the user
    /// settings are loaded. `None` installs none.
    pub log_dir: Option<PathBuf>,
    /// The single-instance listener of this process, polled for forwarded files.
    pub instance: Option<SingleInstance>,
    /// The directory of the automation socket; `None` keeps the automation switch unavailable.
    pub automation_dir: Option<PathBuf>,
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
    let mut app = startup.app;
    let mut shell = Shell::new(&app, startup.config, Some(startup.gpu.clone()));
    shell.start(&mut app, startup.files);
    if let Some(dir) = startup.log_dir {
        match super::install_logging(&app, dir) {
            Ok(ring) => shell.set_log_ring(ring),
            // The shell runs without a log rather than not at all.
            Err(e) => shell.error(e),
        }
    }
    let mut localizer = startup.localizer;
    prefs::apply_language(&app, &mut localizer, &startup.os_locales);
    let mut handler = Handler {
        app,
        shell,
        localizer,
        os_locales: startup.os_locales,
        gpu: startup.gpu,
        title: startup.title,
        states,
        states_path: startup.window_states,
        menu: platform::native_menu(),
        native_menu: false,
        window: None,
        floating: Vec::new(),
        failure: None,
        started: Instant::now(),
        instance: startup.instance,
        automation: Automation::new(),
        automation_dir: startup.automation_dir,
        automation_applied: None,
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
    /// The window identifier: [`MAIN_WINDOW`] or a [`WindowModel::id`].
    key: String,
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
    os_locales: Vec<String>,
    gpu: Gpu,
    title: String,
    states: WindowStates,
    states_path: Option<PathBuf>,
    menu: Box<dyn NativeMenu>,
    native_menu: bool,
    window: Option<Open>,
    /// The secondary windows of the floating dock trees.
    floating: Vec<Open>,
    failure: Option<Error>,
    started: Instant,
    /// The single-instance listener; files other starts forward are opened here.
    instance: Option<SingleInstance>,
    /// The automation socket and the directory it lives in.
    automation: Automation,
    automation_dir: Option<PathBuf>,
    /// The effective value of the automation switch the socket was last applied for.
    automation_applied: Option<bool>,
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

/// The size of the main window when no placement was saved: 1440 by 900 points, at most nine
/// tenths of the primary monitor.
fn default_size(monitor: Option<&Monitor>) -> [f64; 2] {
    const WIDTH: f64 = 1440.0;
    const HEIGHT: f64 = 900.0;
    match monitor {
        Some(m) if m.scale > 0.0 => {
            let [w, h] = m.size.map(|v| f64::from(v) / m.scale * 0.9);
            [WIDTH.min(w.max(640.0)), HEIGHT.min(h.max(480.0))]
        }
        _ => [WIDTH, HEIGHT],
    }
}

impl Handler {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Error) {
        self.failure.get_or_insert(error);
        event_loop.exit();
    }

    /// Opens the main window (`model` is `None`) or the window of a floating tree.
    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        model: Option<&WindowModel>,
    ) -> Result<Open> {
        let (list, primary) = monitors(event_loop);
        let key = model.map_or(MAIN_WINDOW, |m| m.id.as_str()).to_owned();
        let mut attributes = Window::default_attributes().with_title(self.title.clone());
        if let Some(p) = self.states.restore(&list, primary, &key) {
            attributes = attributes
                .with_position(PhysicalPosition::new(p.position[0], p.position[1]))
                .with_inner_size(PhysicalSize::new(p.size[0], p.size[1]))
                .with_maximized(p.maximized);
        } else if let Some(m) = model {
            let [x, y, w, h] = m.rect;
            attributes = attributes
                .with_position(LogicalPosition::new(x, y))
                .with_inner_size(LogicalSize::new(w, h));
        } else {
            let [w, h] = default_size(list.get(primary));
            attributes = attributes.with_inner_size(LogicalSize::new(w, h));
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
        if model.is_none() {
            let entries = self.shell.menu_entries(&self.app, &self.localizer);
            self.native_menu = self.menu.install(&entries);
        }
        let scale = window.scale_factor();
        Ok(Open {
            key,
            window,
            surface,
            config,
            egui,
            renderer,
            textures: BTreeMap::new(),
            scale,
        })
    }

    /// Saves the placement of window `id`, or of every window with `None`.
    fn save_placement(&mut self, event_loop: &ActiveEventLoop, id: Option<WindowId>) {
        let (list, _) = monitors(event_loop);
        let windows = self.window.iter().chain(&self.floating);
        for w in windows.filter(|w| id.is_none_or(|id| w.window.id() == id)) {
            let position = w.window.outer_position().unwrap_or_default();
            let size = w.window.inner_size();
            self.states.save(
                &list,
                &w.key,
                Placement {
                    position: [position.x, position.y],
                    size: [size.width, size.height],
                    maximized: w.window.is_maximized(),
                },
            );
        }
    }

    /// The open window `id`: the main one or a secondary one.
    fn open_mut(&mut self, id: WindowId) -> Option<&mut Open> {
        if self.window.as_ref().is_some_and(|w| w.window.id() == id) {
            return self.window.as_mut();
        }
        self.floating.iter_mut().find(|w| w.window.id() == id)
    }

    /// Opens a window for every floating tree that has none and closes the windows of trees
    /// that are gone.
    fn sync_windows(&mut self, event_loop: &ActiveEventLoop) {
        let models = self.shell.windows();
        self.floating
            .retain(|w| models.iter().any(|m| m.id == w.key));
        for model in &models {
            if self.floating.iter().any(|w| w.key == model.id) {
                continue;
            }
            match self.open_window(event_loop, Some(model)) {
                Ok(w) => {
                    w.window.request_redraw();
                    self.floating.push(w);
                }
                Err(e) => {
                    // The tree goes back into the main window rather than staying invisible.
                    self.shell.dock_window(&model.id);
                    self.shell.error(e);
                }
            }
        }
    }

    fn write_states(&self) {
        if let Some(path) = &self.states_path {
            // A placement that cannot be written is not worth stopping the shutdown for.
            let _ = self.states.save_to(path);
        }
    }

    /// Copies new viewport frames into textures the egui renderer of window `w` samples.
    fn register_frames(gpu: &Gpu, shell: &Shell, w: &mut Open, encoder: &mut wgpu::CommandEncoder) {
        let device = gpu.device();
        let frames = shell.viewport_frames();
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

    fn redraw(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let device = self.gpu.device().clone();
        let queue = self.gpu.queue().clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("arc-app frame"),
        });
        let main = self.window.as_ref().is_some_and(|w| w.window.id() == id);
        let (gpu, shell) = (self.gpu.clone(), &self.shell);
        let Some(w) = (if main {
            self.window.as_mut()
        } else {
            self.floating.iter_mut().find(|w| w.window.id() == id)
        }) else {
            return;
        };
        Self::register_frames(&gpu, shell, w, &mut encoder);
        let raw = w.egui.take_egui_input(&w.window);
        let ctx = w.egui.egui_ctx().clone();
        let textures: BTreeMap<TabId, egui::TextureId> =
            w.textures.iter().map(|(t, r)| (*t, r.id)).collect();
        let input = FrameInput {
            raw,
            time: self.started.elapsed().as_secs_f64(),
            localize: &self.localizer,
            language: self.localizer.active().language.as_str(),
            textures: &textures,
            menu_intents: if main { self.menu.poll() } else { Vec::new() },
            native_menu: self.native_menu,
        };
        let out = if main {
            self.shell.frame(&mut self.app, &ctx, input)
        } else {
            let key = w.key.clone();
            self.shell.frame_window(&mut self.app, &ctx, input, &key)
        };
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
        self.after_frame(event_loop, out.settings_changed, &out.requests);
    }

    /// Applies a changed language, carries out the frame's requests and opens or closes the
    /// windows of floating trees.
    fn after_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        settings_changed: bool,
        requests: &[Request],
    ) {
        if settings_changed {
            prefs::apply_language(&self.app, &mut self.localizer, &self.os_locales);
            self.request_redraw_all();
        }
        for request in requests {
            self.carry_out(event_loop, request);
        }
        self.sync_windows(event_loop);
    }

    fn request_redraw_all(&self) {
        for w in self.window.iter().chain(&self.floating) {
            w.window.request_redraw();
        }
    }

    fn carry_out(&mut self, event_loop: &ActiveEventLoop, request: &Request) {
        let mut dialogs = NativeDialogs {
            localize: Some(&self.localizer),
        };
        let dialogs: &mut dyn FileDialogs = &mut dialogs;
        match platform::carry_out(&mut self.shell, &mut self.app, dialogs, request) {
            Ok(Done::Quit) => {
                self.save_placement(event_loop, None);
                self.write_states();
                self.shell.save_recent(&self.app);
                event_loop.exit();
            }
            Ok(_) => {}
            Err(e) => self.shell.error(e),
        }
        self.shell.save_recent(&self.app);
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
        match self.open_window(event_loop, None) {
            Ok(w) => {
                w.window.request_redraw();
                self.window = Some(w);
            }
            Err(e) => self.fail(event_loop, e),
        }
        self.sync_windows(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let main = self.window.as_ref().is_some_and(|w| w.window.id() == id);
        let device = self.gpu.device().clone();
        let Some(w) = self.open_mut(id) else {
            return;
        };
        let response = w.egui.on_window_event(&w.window, &event);
        match event {
            WindowEvent::CloseRequested if main => self.carry_out(event_loop, &Request::Quit),
            WindowEvent::CloseRequested => {
                // Closing a floating window docks its panels back into the main window.
                let key = w.key.clone();
                self.save_placement(event_loop, Some(id));
                self.shell.dock_window(&key);
                self.sync_windows(event_loop);
                self.request_redraw_all();
            }
            WindowEvent::Resized(size) => {
                w.config.width = size.width.max(1);
                w.config.height = size.height.max(1);
                w.surface.configure(&device, &w.config);
                w.window.request_redraw();
                self.save_placement(event_loop, Some(id));
            }
            WindowEvent::Moved(_) => self.save_placement(event_loop, Some(id)),
            WindowEvent::ScaleFactorChanged {
                scale_factor,
                mut inner_size_writer,
            } => {
                let size = w.window.inner_size();
                let [width, height] = rescale([size.width, size.height], w.scale, scale_factor);
                w.scale = scale_factor;
                let _ = inner_size_writer.request_inner_size(PhysicalSize::new(width, height));
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop, id),
            _ => {
                if response.repaint {
                    w.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.receive_forwarded();
        self.apply_automation();
        if self.shell.busy() {
            self.request_redraw_all();
            event_loop.set_control_flow(ControlFlow::Poll);
        } else if self.instance.is_some() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FORWARD_POLL));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl Handler {
    /// Opens the files other starts forwarded, through the application's open flow. Paths are
    /// not logged.
    fn receive_forwarded(&mut self) {
        let paths = match &mut self.instance {
            Some(instance) => instance.poll(Instant::now()),
            None => return,
        };
        if paths.is_empty() {
            return;
        }
        for path in &paths {
            if let Err(e) = self.app.open(path) {
                self.shell.error(e);
            }
        }
        self.request_redraw_all();
    }

    /// Starts or stops the automation socket when the effective value of its switch changed since
    /// the last attempt. A failure is shown once; the next change tries again.
    fn apply_automation(&mut self) {
        let Some(dir) = &self.automation_dir else {
            return;
        };
        let enabled = matches!(
            self.app
                .settings()
                .get(automation::ENABLED)
                .map(|e| e.value),
            Ok(SettingValue::Bool(true))
        );
        if self.automation_applied == Some(enabled) {
            return;
        }
        self.automation_applied = Some(enabled);
        let product = self.app.product();
        if let Err(e) = self.automation.sync(
            self.app.settings(),
            dir,
            product.headless(),
            product.view_renderer(),
        ) {
            self.shell.error(e);
        }
    }
}
