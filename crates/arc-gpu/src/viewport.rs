//! Off-thread rendering (CORE-GPU-029) with on-demand redraw (CORE-GPU-030).
//!
//! A [`Viewport`] owns a render thread, a [`Scene`] and a [`Renderer`]. The caller's calls only
//! record inputs in a mailbox under a short lock and wake the thread; they never wait for a
//! frame. The mailbox holds the latest value of each input (camera, size, settings, highlight,
//! overlay), one pending extraction result per object (the one with the greatest stamp; failed
//! results are not queued, since the driver already delivered their error) and the reasons to
//! draw, so it stays bounded however fast the caller sends: a camera sent while a frame renders
//! replaces the previous pending camera.
//!
//! The thread draws only when something changed since the last frame: an input set to a value
//! different from its current one, an extraction result that changed the scene, or a requested
//! progressive frame. Each frame takes every change pending when it starts; an idle viewport
//! waits on a condition variable and does no GPU work. [`Frame::reasons`] says why a frame was
//! drawn and [`Viewport::frames`] counts frames.
//!
//! Errors on the thread (rejected extraction results, device errors) are returned by the next
//! call, which still records its input. After `gpu.device.lost` the thread stops and every call
//! returns that code. Dropping the viewport asks the thread to stop and waits for it at most
//! [`JOIN_TIMEOUT`]; a thread still finishing a frame after that is detached and exits after
//! the frame.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arc_base::{Error, ErrorCode, ObjectId, Result};
use arc_geom::view::ViewFrame;

use crate::adapter::Gpu;
use crate::display::{DisplayOverrides, Section};
use crate::error::codes;
use crate::extract::ExtractionResult;
use crate::manipulator::Manipulator;
use crate::overlay::Overlay;
use crate::quality::{AdaptiveQuality, SETTLE_TIME};
use crate::render::{FrameExtras, Image, RenderSettings, Renderer};
use crate::scene::{Scene, SceneLimits};

/// How long dropping a [`Viewport`] waits for its render thread.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(10);

/// The clock of the adaptive quality rules: the time since an origin the clock chooses.
pub type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

/// The initial state of a [`Viewport`].
#[derive(Clone)]
pub struct ViewportConfig {
    /// The target width in pixels.
    pub width: u32,
    /// The target height in pixels.
    pub height: u32,
    /// The camera.
    pub frame: ViewFrame,
    /// The render settings.
    pub settings: RenderSettings,
    /// The limits of the viewport's scene.
    pub limits: SceneLimits,
    /// Whether each frame is also read back into [`Frame::image`] (headless use). Without it
    /// frames carry only the texture, for a compositor on the same device.
    pub readback: bool,
    /// The time the camera must stay still before full quality returns (CORE-GPU-021).
    pub settle: Duration,
    /// The clock the settle time is measured with. `None` reads the time since the render
    /// thread started; tests inject a clock they advance by hand.
    pub clock: Option<Clock>,
}

impl fmt::Debug for ViewportConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewportConfig")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame", &self.frame)
            .field("settings", &self.settings)
            .field("limits", &self.limits)
            .field("readback", &self.readback)
            .field("settle", &self.settle)
            .finish_non_exhaustive()
    }
}

impl ViewportConfig {
    /// A configuration with default settings and limits and readback on.
    #[must_use]
    pub fn new(width: u32, height: u32, frame: ViewFrame) -> Self {
        Self {
            width,
            height,
            frame,
            settings: RenderSettings::DEFAULT,
            limits: SceneLimits::DEFAULT,
            readback: true,
            settle: SETTLE_TIME,
            clock: None,
        }
    }
}

/// Why a frame was drawn, or what is pending.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One flag per kind of change.
pub struct DirtyReasons {
    /// The camera changed.
    pub camera: bool,
    /// The target size changed.
    pub size: bool,
    /// Extraction results changed the scene.
    pub scene: bool,
    /// The render settings changed.
    pub settings: bool,
    /// The hovered or selected objects changed.
    pub highlight: bool,
    /// The overlay changed.
    pub overlay: bool,
    /// The display overrides changed.
    pub overrides: bool,
    /// The section changed.
    pub section: bool,
    /// The manipulator changed.
    pub manipulator: bool,
    /// A progressive frame was requested.
    pub progressive: bool,
    /// Full quality returned because the camera has been still for the settle time
    /// (CORE-GPU-021). Set by the render thread, not by a call.
    pub quality: bool,
}

impl DirtyReasons {
    /// Whether any reason is set.
    #[must_use]
    pub fn any(&self) -> bool {
        self.camera
            || self.size
            || self.scene
            || self.settings
            || self.highlight
            || self.overlay
            || self.overrides
            || self.section
            || self.manipulator
            || self.progressive
            || self.quality
    }
}

/// A finished frame.
#[derive(Debug)]
pub struct Frame {
    /// The frame number: 1 for the first frame, one more for each later one.
    pub number: u64,
    /// Why it was drawn.
    pub reasons: DirtyReasons,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The color target (`Rgba8UnormSrgb`, usable as a texture binding and copy source). Each
    /// frame has its own texture, so a compositor can sample it while the next frame renders.
    pub texture: wgpu::Texture,
    /// A view of the whole texture.
    pub view: wgpu::TextureView,
    /// The read-back image when [`ViewportConfig::readback`] is set.
    pub image: Option<Image>,
}

/// A viewport rendered on its own thread; see the [module documentation](self).
pub struct Viewport {
    shared: Arc<Shared>,
    limits: SceneLimits,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for Viewport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Viewport")
            .field("frames", &self.frames())
            .finish_non_exhaustive()
    }
}

struct Inputs {
    frame: ViewFrame,
    size: (u32, u32),
    settings: RenderSettings,
    hovered: Option<ObjectId>,
    selected: Vec<ObjectId>,
    overlay: Option<Overlay>,
    overrides: Option<DisplayOverrides>,
    section: Option<Section>,
    manipulator: Option<Manipulator>,
}

/// Work run on the render thread with its scene, renderer and settings.
type Job = Box<dyn FnOnce(&mut Renderer, &mut Scene, &RenderSettings) + Send>;

struct State {
    inputs: Inputs,
    jobs: Vec<Job>,
    dirty: DirtyReasons,
    scene: std::collections::HashMap<ObjectId, ExtractionResult>,
    /// Calls that may need work, and how many of them the thread has handled.
    requested: u64,
    processed: u64,
    frames: u64,
    latest: Option<Arc<Frame>>,
    error: Option<Error>,
    stopped: Option<ErrorCode>,
    shutdown: bool,
    exited: bool,
    join_timeout: Duration,
    #[cfg(test)]
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the render thread.
    wake: Condvar,
    /// Signals handled calls, finished frames and the thread's exit.
    done: Condvar,
}

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Viewport {
    /// Creates the scene and renderer on `gpu` and starts the render thread, which draws the
    /// first frame.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` and `gpu.limit.exceeded` for invalid settings or size; scene
    /// creation errors; `gpu.thread.failed` when the thread cannot be started.
    pub fn new(gpu: &Gpu, config: ViewportConfig) -> Result<Self> {
        config.settings.validate()?;
        check_size(&config.limits, config.width, config.height)?;
        let scene = Scene::new(gpu, config.limits)?;
        let renderer = Renderer::new(gpu);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                inputs: Inputs {
                    frame: config.frame,
                    size: (config.width, config.height),
                    settings: config.settings,
                    hovered: None,
                    selected: Vec::new(),
                    overlay: None,
                    overrides: None,
                    section: None,
                    manipulator: None,
                },
                dirty: DirtyReasons {
                    camera: true,
                    size: true,
                    settings: true,
                    ..DirtyReasons::default()
                },
                jobs: Vec::new(),
                scene: std::collections::HashMap::new(),
                requested: 1,
                processed: 0,
                frames: 0,
                latest: None,
                error: None,
                stopped: None,
                shutdown: false,
                exited: false,
                join_timeout: JOIN_TIMEOUT,
                #[cfg(test)]
                hook: None,
            }),
            wake: Condvar::new(),
            done: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        let readback = config.readback;
        let settle = config.settle;
        let clock: Clock = match config.clock {
            Some(clock) => clock,
            None => {
                let start = Instant::now();
                Arc::new(move || start.elapsed())
            }
        };
        let thread = std::thread::Builder::new()
            .name("arc-gpu viewport".into())
            .spawn(move || {
                let guard = ExitGuard(thread_shared);
                render_loop(&guard.0, scene, renderer, readback, settle, &clock);
            })
            .map_err(|e| Error::new(codes::THREAD_FAILED).with_source(e))?;
        Ok(Self {
            shared,
            limits: config.limits,
            thread: Some(thread),
        })
    }

    /// Wakes the render thread so it reads the clock again. A test that advances an injected
    /// clock calls it; the thread draws only if the settled camera then needs a frame.
    #[cfg(test)]
    pub(crate) fn wake(&self) {
        let guard = lock(&self.shared);
        self.shared.wake.notify_all();
        drop(guard);
    }

    /// Records an input: `change` returns whether it changed anything the thread must handle.
    fn send(&self, change: impl FnOnce(&mut State) -> bool) -> Result<()> {
        let mut state = lock(&self.shared);
        if let Some(code) = state.stopped {
            return Err(Error::new(code));
        }
        let error = state.error.take();
        if change(&mut state) {
            state.requested += 1;
            self.shared.wake.notify_one();
        }
        error.map_or(Ok(()), Err)
    }

    /// Sets the camera; the latest one wins.
    ///
    /// # Errors
    ///
    /// An earlier error of the render thread.
    pub fn set_camera(&self, frame: ViewFrame) -> Result<()> {
        self.send(|s| {
            let changed = s.inputs.frame != frame;
            s.inputs.frame = frame;
            s.dirty.camera |= changed;
            changed
        })
    }

    /// Sets the target size.
    ///
    /// # Errors
    ///
    /// `gpu.limit.exceeded` for a zero side or one above `max_image_side` (nothing is
    /// recorded); otherwise an earlier error of the render thread.
    pub fn resize(&self, width: u32, height: u32) -> Result<()> {
        check_size(&self.limits, width, height)?;
        self.send(|s| {
            let changed = s.inputs.size != (width, height);
            s.inputs.size = (width, height);
            s.dirty.size |= changed;
            changed
        })
    }

    /// Sets the render settings.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` (nothing is recorded); otherwise an earlier error of the render
    /// thread.
    pub fn set_settings(&self, settings: RenderSettings) -> Result<()> {
        settings.validate()?;
        self.send(|s| {
            let changed = s.inputs.settings != settings;
            s.inputs.settings = settings;
            s.dirty.settings |= changed;
            changed
        })
    }

    /// Sets the hovered and selected objects. The selection is compared as a set.
    ///
    /// # Errors
    ///
    /// `gpu.limit.exceeded` for more selected objects than `max_objects` (nothing is
    /// recorded); otherwise an earlier error of the render thread.
    pub fn set_highlight(&self, hovered: Option<ObjectId>, selected: &[ObjectId]) -> Result<()> {
        if selected.len() > usize::try_from(self.limits.max_objects).unwrap_or(usize::MAX) {
            return Err(Error::new(codes::LIMIT_EXCEEDED));
        }
        let mut selected = selected.to_vec();
        selected.sort_unstable();
        selected.dedup();
        self.send(|s| {
            let changed = s.inputs.hovered != hovered || s.inputs.selected != selected;
            s.inputs.hovered = hovered;
            s.inputs.selected = selected;
            s.dirty.highlight |= changed;
            changed
        })
    }

    /// Sets or clears the overlay.
    ///
    /// # Errors
    ///
    /// An earlier error of the render thread. Overlay errors surface from the frame that
    /// resolves it.
    pub fn set_overlay(&self, overlay: Option<Overlay>) -> Result<()> {
        self.send(|s| {
            let changed = s.inputs.overlay != overlay;
            s.inputs.overlay = overlay;
            s.dirty.overlay |= changed;
            changed
        })
    }

    /// Sets or clears the display overrides of the view.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` and `gpu.limit.exceeded` for invalid overrides (nothing is
    /// recorded); otherwise an earlier error of the render thread.
    pub fn set_overrides(&self, overrides: Option<DisplayOverrides>) -> Result<()> {
        if let Some(o) = &overrides {
            o.validate(self.limits.max_objects)?;
        }
        self.send(|s| {
            let changed = s.inputs.overrides != overrides;
            s.inputs.overrides = overrides;
            s.dirty.overrides |= changed;
            changed
        })
    }

    /// Sets or clears the section of the view. An invalid section surfaces from the frame that
    /// resolves it.
    ///
    /// # Errors
    ///
    /// An earlier error of the render thread.
    pub fn set_section(&self, section: Option<Section>) -> Result<()> {
        self.send(|s| {
            let changed = s.inputs.section != section;
            s.inputs.section = section;
            s.dirty.section |= changed;
            changed
        })
    }

    /// Sets or clears the manipulator. An invalid manipulator surfaces from the frame that
    /// draws it.
    ///
    /// # Errors
    ///
    /// An earlier error of the render thread.
    pub fn set_manipulator(&self, manipulator: Option<Manipulator>) -> Result<()> {
        self.send(|s| {
            let changed = s.inputs.manipulator != manipulator;
            s.inputs.manipulator = manipulator;
            s.dirty.manipulator |= changed;
            changed
        })
    }

    /// Queues extraction results for the viewport's scene (see
    /// [`Scene::apply_extractions`]). Per object only the result with the greatest stamp is
    /// kept; failed results are skipped.
    ///
    /// # Errors
    ///
    /// `gpu.limit.exceeded` when more than `max_objects` objects would be pending (nothing is
    /// recorded); otherwise an earlier error of the render thread.
    pub fn update_scene(&self, results: Vec<ExtractionResult>) -> Result<()> {
        let limit = usize::try_from(self.limits.max_objects).unwrap_or(usize::MAX);
        let mut too_many = false;
        self.send(|s| {
            let objects: std::collections::HashSet<ObjectId> =
                results.iter().map(|r| r.object).collect();
            let new = objects.iter().filter(|o| !s.scene.contains_key(o)).count();
            if s.scene.len().saturating_add(new) > limit {
                too_many = true;
                return false;
            }
            let mut changed = false;
            for result in results.into_iter().filter(|r| !r.is_failed()) {
                let newer = s
                    .scene
                    .get(&result.object)
                    .is_none_or(|p| p.stamp() < result.stamp());
                if newer {
                    s.scene.insert(result.object, result);
                    changed = true;
                }
            }
            changed
        })?;
        if too_many {
            return Err(Error::new(codes::LIMIT_EXCEEDED));
        }
        Ok(())
    }

    /// Requests one more frame for a progressive effect.
    ///
    /// # Errors
    ///
    /// An earlier error of the render thread.
    pub fn request_progressive_frame(&self) -> Result<()> {
        self.send(|s| {
            s.dirty.progressive = true;
            true
        })
    }

    /// Runs `job` on the render thread with the viewport's renderer, scene (with every
    /// extraction result queued before this call applied) and the render settings of the frame
    /// that is drawn at that time, lowered while the camera moves (see
    /// [`AdaptiveQuality`]), and waits at most `timeout` for its result. Picking a viewport's
    /// scene ([`Renderer::pick`], [`Scene::bounding_sphere`]) goes through this, so a pick
    /// during navigation tests the same levels of detail as the frame on screen. The job runs
    /// between frames; it draws nothing by itself.
    ///
    /// # Errors
    ///
    /// `gpu.timeout` when the job has not finished in time (it still runs later and its result
    /// is dropped); an earlier error of the render thread.
    pub fn with_scene<R: Send + 'static>(
        &self,
        timeout: Duration,
        job: impl FnOnce(&mut Renderer, &mut Scene, &RenderSettings) -> R + Send + 'static,
    ) -> Result<R> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.send(|s| {
            s.jobs.push(Box::new(move |renderer, scene, settings| {
                let _ = sender.send(job(renderer, scene, settings));
            }));
            true
        })?;
        receiver.recv_timeout(timeout).map_err(|_| {
            lock(&self.shared)
                .stopped
                .map_or_else(|| Error::new(codes::TIMEOUT), Error::new)
        })
    }

    /// The number of frames drawn.
    #[must_use]
    pub fn frames(&self) -> u64 {
        lock(&self.shared).frames
    }

    /// The reasons recorded since the last frame started, not yet drawn. Scene changes appear
    /// only once the thread has applied them.
    #[must_use]
    pub fn dirty(&self) -> DirtyReasons {
        lock(&self.shared).dirty
    }

    /// The latest finished frame, without waiting.
    #[must_use]
    pub fn latest_frame(&self) -> Option<Arc<Frame>> {
        lock(&self.shared).latest.clone()
    }

    /// Waits at most `timeout` until a frame numbered above `after` exists and returns the
    /// latest frame.
    ///
    /// # Errors
    ///
    /// `gpu.timeout` when none arrives in time; an earlier error of the render thread.
    pub fn wait_frame(&self, after: u64, timeout: Duration) -> Result<Arc<Frame>> {
        let mut state = self.wait(timeout, |s, _| s.frames > after || s.error.is_some())?;
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        state
            .latest
            .clone()
            .ok_or_else(|| Error::new(codes::TIMEOUT))
    }

    /// Waits at most `timeout` until the thread has handled every call made before this one,
    /// including the frame they cause.
    ///
    /// # Errors
    ///
    /// `gpu.timeout` when the thread does not catch up in time; an earlier error of the render
    /// thread.
    pub fn sync(&self, timeout: Duration) -> Result<()> {
        let mut state = self.wait(timeout, |s, target| s.processed >= target)?;
        state.error.take().map_or(Ok(()), Err)
    }

    fn wait(
        &self,
        timeout: Duration,
        ready: impl Fn(&State, u64) -> bool,
    ) -> Result<MutexGuard<'_, State>> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared);
        let target = state.requested;
        loop {
            if let Some(code) = state.stopped {
                return Err(Error::new(code));
            }
            // Calls made while waiting are not waited for.
            if ready(&state, target) {
                return Ok(state);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(Error::new(codes::TIMEOUT));
            };
            state = self
                .shared
                .done
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Viewport {
    fn drop(&mut self) {
        let mut state = lock(&self.shared);
        state.shutdown = true;
        self.shared.wake.notify_all();
        let deadline = Instant::now() + state.join_timeout;
        while !state.exited {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            state = self
                .shared
                .done
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        let exited = state.exited;
        drop(state);
        // A thread that has not exited is detached by dropping its handle.
        if let (true, Some(thread)) = (exited, self.thread.take()) {
            let _ = thread.join();
        }
    }
}

fn check_size(limits: &SceneLimits, width: u32, height: u32) -> Result<()> {
    let side = 1..=limits.max_image_side;
    if side.contains(&width) && side.contains(&height) {
        Ok(())
    } else {
        Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_image_side"))
    }
}

/// Marks the thread as exited however it ends, so callers and `drop` stop waiting.
struct ExitGuard(Arc<Shared>);

impl Drop for ExitGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.0);
        state.exited = true;
        state.stopped.get_or_insert(codes::THREAD_FAILED);
        self.0.done.notify_all();
    }
}

#[allow(clippy::too_many_lines)] // The loop: wait, take the inputs, apply, draw, publish.
fn render_loop(
    shared: &Shared,
    mut scene: Scene,
    mut renderer: Renderer,
    readback: bool,
    settle: Duration,
    clock: &Clock,
) {
    let mut quality = AdaptiveQuality::new(settle);
    // Whether the last frame was drawn at navigation quality.
    let mut navigating = false;
    loop {
        let (inputs, mut reasons, results, jobs, generation, moved) = {
            let mut state = lock(shared);
            loop {
                if state.shutdown {
                    return;
                }
                let now = clock();
                // Navigation has settled since the last frame: draw at full quality.
                let settled = navigating && !quality.is_navigating(now);
                if state.dirty.any() || !state.scene.is_empty() || !state.jobs.is_empty() || settled
                {
                    break;
                }
                state.processed = state.requested;
                shared.done.notify_all();
                state = match quality.settled_at().filter(|_| navigating) {
                    Some(at) => {
                        shared
                            .wake
                            .wait_timeout(state, at.saturating_sub(now))
                            .unwrap_or_else(PoisonError::into_inner)
                            .0
                    }
                    None => shared
                        .wake
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner),
                };
            }
            let i = &state.inputs;
            let inputs = Inputs {
                frame: i.frame,
                size: i.size,
                settings: i.settings,
                hovered: i.hovered,
                selected: i.selected.clone(),
                overlay: i.overlay.clone(),
                overrides: i.overrides.clone(),
                section: i.section.clone(),
                manipulator: i.manipulator,
            };
            let reasons = std::mem::take(&mut state.dirty);
            let results: Vec<_> = std::mem::take(&mut state.scene).into_values().collect();
            let jobs = std::mem::take(&mut state.jobs);
            // A camera change after the first frame is navigation.
            let moved = reasons.camera && state.frames > 0;
            (inputs, reasons, results, jobs, state.requested, moved)
        };
        let now = clock();
        if moved {
            quality.camera_moved(now);
        }
        let now_navigating = quality.is_navigating(now);
        // Full quality returns with a frame of its own once the camera has settled.
        reasons.quality |= navigating && !now_navigating;
        navigating = now_navigating;
        let settings = quality.apply(&inputs.settings, now);
        let mut error = None;
        if !results.is_empty() {
            let report = scene.apply_extractions(results);
            reasons.scene |= report.applied > 0;
            error = report.failed.into_iter().next().map(|(_, e)| e);
        }
        for job in jobs {
            job(&mut renderer, &mut scene, &settings);
        }
        let mut drawn = None;
        if reasons.any() && !scene.gpu().is_lost() {
            #[cfg(test)]
            {
                let hook = lock(shared).hook.clone();
                if let Some(hook) = hook {
                    hook();
                }
            }
            let extras = FrameExtras {
                hovered: inputs.hovered,
                selected: &inputs.selected,
                overlay: inputs.overlay.as_ref(),
                overrides: inputs.overrides.as_ref(),
                section: inputs.section.as_ref(),
                manipulator: inputs.manipulator.as_ref(),
            };
            match renderer.draw(
                &mut scene,
                &inputs.frame,
                inputs.size,
                &settings,
                &extras,
                readback,
            ) {
                Ok(frame) => drawn = Some(frame),
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        let mut state = lock(shared);
        if let Some((texture, rgba8)) = drawn {
            state.frames += 1;
            let (width, height) = inputs.size;
            state.latest = Some(Arc::new(Frame {
                number: state.frames,
                reasons,
                width,
                height,
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                texture,
                image: rgba8.map(|rgba8| Image {
                    width,
                    height,
                    rgba8,
                }),
            }));
        }
        if let Some(error) = error {
            state.error.get_or_insert(error);
        }
        state.processed = generation;
        if scene.gpu().is_lost() {
            state.stopped = Some(codes::DEVICE_LOST);
            return;
        }
        shared.done.notify_all();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "tests fail by panicking")]

    use arc_base::id::Identifier;
    use std::sync::mpsc;

    use arc_base::Color;
    use arc_base::math::{Point3, Vector3};
    use arc_geom::view::Projection;

    use super::*;
    use crate::adapter::GpuSettings;
    use crate::extract::{Extraction, Outcome};
    use crate::overlay::{Anchor, MarkerShape, OverlayPrimitive};
    use crate::quality::NAVIGATION_LOD_FACTOR;
    use crate::scene::{LineItem, LinePath, LineStyle, LineWidth, RenderItem};

    /// Generous: bounds only a hung thread on a loaded machine.
    const WAIT: Duration = Duration::from_secs(60);
    /// A settle time no test reaches: without a clock the thread never draws a settle frame.
    const NEVER: Duration = Duration::from_secs(1 << 30);

    /// A clock the test moves by hand; the render thread reads it through [`Clock`].
    #[derive(Clone, Default)]
    struct ManualClock(Arc<Mutex<Duration>>);

    impl ManualClock {
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) += by;
        }

        fn reader(&self) -> Clock {
            let now = Arc::clone(&self.0);
            Arc::new(move || *now.lock().unwrap_or_else(PoisonError::into_inner))
        }
    }

    fn gpu() -> Gpu {
        Gpu::new(&GpuSettings::default()).unwrap()
    }

    fn view(height: f64) -> ViewFrame {
        ViewFrame::new(
            Point3::new(0.0, 0.0, 100.0),
            Point3::origin(),
            Vector3::y(),
            Projection::Orthographic { height },
            1.0,
            1000.0,
        )
        .unwrap()
    }

    fn line(object: u128, revision: u64, x: f64) -> ExtractionResult {
        ExtractionResult {
            object: ObjectId::from_u128(object),
            revision,
            sequence: 0,
            outcome: Outcome::Items(Extraction {
                geometries: Vec::new(),
                lods: Vec::new(),
                items: vec![RenderItem::Lines(LineItem {
                    origin: Point3::new(x, 0.0, 0.0),
                    paths: vec![LinePath {
                        points: vec![[-3.0, 0.0, 0.0], [3.0, 0.0, 0.0]],
                        closed: false,
                    }],
                    style: LineStyle::round(LineWidth::Screen(4.0)),
                    color: Color::WHITE,
                })],
            }),
        }
    }

    fn viewport(readback: bool) -> Viewport {
        let mut config = ViewportConfig::new(32, 32, view(10.0));
        config.readback = readback;
        config.settle = NEVER;
        let viewport = Viewport::new(&gpu(), config).unwrap();
        viewport.sync(WAIT).unwrap();
        assert_eq!(viewport.frames(), 1);
        viewport
    }

    /// A viewport whose settle time is the default one, measured by `clock`.
    fn viewport_with_clock(clock: &ManualClock) -> Viewport {
        let mut config = ViewportConfig::new(32, 32, view(10.0));
        config.readback = false;
        config.settle = SETTLE_TIME;
        config.clock = Some(clock.reader());
        let viewport = Viewport::new(&gpu(), config).unwrap();
        viewport.sync(WAIT).unwrap();
        assert_eq!(viewport.frames(), 1);
        viewport
    }

    /// A hook that announces each frame start and blocks it until released.
    fn hold(viewport: &Viewport) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (started, on_start) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let (started, released) = (Mutex::new(started), Mutex::new(released));
        lock(&viewport.shared).hook = Some(Arc::new(move || {
            let _ = started.lock().unwrap().send(());
            let _ = released.lock().unwrap().recv_timeout(WAIT);
        }));
        (on_start, release)
    }

    #[test]
    fn jobs_see_the_queued_scene_on_the_render_thread() {
        let viewport = viewport(false);
        viewport.update_scene(vec![line(7, 1, 0.0)]).unwrap();
        let frame = view(10.0);
        let hits = viewport
            .with_scene(WAIT, move |renderer, scene, settings| {
                let region = crate::render::PickRegion::pixel(16, 16, 2.0);
                let hits = renderer.pick(scene, &frame, 32, 32, settings, &region);
                (hits, scene.bounding_sphere(None))
            })
            .unwrap();
        let (hits, bounds) = hits;
        let hits = hits.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key.object, ObjectId::from_u128(7));
        assert!(bounds.is_some());
    }

    #[test]
    fn frames_carry_the_image_and_the_texture() {
        let viewport = viewport(true);
        viewport.update_scene(vec![line(1, 1, 0.0)]).unwrap();
        let frame = viewport.wait_frame(1, WAIT).unwrap();
        assert_eq!(frame.number, 2);
        let image = frame.image.as_ref().unwrap();
        assert_eq!(image.pixel(16, 16), Some([255, 255, 255, 255]));
        assert_eq!(image.pixel(16, 2), Some([0, 0, 0, 255]));
        assert_eq!(frame.texture.width(), 32);

        let compositor = viewport_without_readback();
        assert!(compositor.latest_frame().unwrap().image.is_none());
    }

    fn viewport_without_readback() -> Viewport {
        viewport(false)
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One case per kind of input.
    fn an_idle_viewport_draws_nothing_and_each_change_draws_one_frame() {
        let clock = ManualClock::default();
        let viewport = viewport_with_clock(&clock);
        let only = |f: fn(&mut DirtyReasons)| {
            let mut r = DirtyReasons::default();
            f(&mut r);
            r
        };
        let expect = |change: &dyn Fn(&Viewport), reasons: DirtyReasons| {
            let before = viewport.frames();
            change(&viewport);
            viewport.sync(WAIT).unwrap();
            assert_eq!(viewport.frames(), before + 1, "{reasons:?}");
            assert_eq!(viewport.latest_frame().unwrap().reasons, reasons);
            // Idle: neither the same input again nor waiting draws anything.
            change(&viewport);
            viewport.sync(WAIT).unwrap();
            viewport.sync(WAIT).unwrap();
            if !reasons.progressive {
                assert_eq!(viewport.frames(), before + 1, "{reasons:?} repeated");
            }
            // A camera change ends with one frame at full quality once the camera is still.
            if reasons.camera {
                clock.advance(SETTLE_TIME);
                viewport.wake();
                let settled = viewport.wait_frame(viewport.frames(), WAIT).unwrap();
                assert_eq!(
                    settled.reasons,
                    DirtyReasons {
                        quality: true,
                        ..DirtyReasons::default()
                    }
                );
            }
        };
        expect(
            &|v| v.set_camera(view(12.0)).unwrap(),
            only(|r| r.camera = true),
        );
        expect(&|v| v.resize(40, 24).unwrap(), only(|r| r.size = true));
        expect(
            &|v| {
                v.set_settings(RenderSettings {
                    ambient: 0.5,
                    ..RenderSettings::DEFAULT
                })
                .unwrap();
            },
            only(|r| r.settings = true),
        );
        expect(
            &|v| v.update_scene(vec![line(1, 1, 0.0)]).unwrap(),
            only(|r| r.scene = true),
        );
        expect(
            &|v| v.set_highlight(None, &[ObjectId::from_u128(1)]).unwrap(),
            only(|r| r.highlight = true),
        );
        let mut overlay = Overlay::new();
        overlay.primitives.push(OverlayPrimitive::Marker {
            at: Anchor::Screen([4.0, 4.0]),
            shape: MarkerShape::Square,
            size_px: 4.0,
            filled: true,
            width_px: 1.0,
            color: Color::WHITE,
        });
        expect(
            &move |v| v.set_overlay(Some(overlay.clone())).unwrap(),
            only(|r| r.overlay = true),
        );
        expect(
            &|v| {
                v.set_overrides(Some(DisplayOverrides {
                    categories: vec![(1, crate::OverrideStyle::hidden())],
                    ..DisplayOverrides::default()
                }))
                .unwrap();
            },
            only(|r| r.overrides = true),
        );
        expect(
            &|v| {
                v.set_section(Some(Section {
                    planes: vec![crate::SectionPlane {
                        point: Point3::origin(),
                        normal: Vector3::z(),
                    }],
                    ..Section::default()
                }))
                .unwrap();
            },
            only(|r| r.section = true),
        );
        expect(
            &|v| {
                v.set_manipulator(Some(Manipulator::new(
                    crate::ManipulatorKind::Move,
                    Point3::origin(),
                )))
                .unwrap();
            },
            only(|r| r.manipulator = true),
        );
        assert!(
            viewport
                .set_overrides(Some(DisplayOverrides {
                    categories: vec![(16, crate::OverrideStyle::hidden())],
                    ..DisplayOverrides::default()
                }))
                .is_err(),
            "invalid overrides are rejected at the call"
        );
        let before = viewport.frames();
        viewport.request_progressive_frame().unwrap();
        viewport.sync(WAIT).unwrap();
        assert_eq!(viewport.frames(), before + 1);
        assert_eq!(
            viewport.latest_frame().unwrap().reasons,
            only(|r| r.progressive = true)
        );
        // A stale extraction result changes nothing and draws nothing.
        viewport.update_scene(vec![line(1, 1, 5.0)]).unwrap();
        viewport.sync(WAIT).unwrap();
        assert_eq!(viewport.frames(), before + 1);
        assert_eq!(viewport.dirty(), DirtyReasons::default());
    }

    #[test]
    fn a_slow_frame_does_not_block_the_caller_and_cameras_coalesce() {
        let viewport = viewport(false);
        let (started, release) = hold(&viewport);
        viewport.set_camera(view(11.0)).unwrap();
        started.recv_timeout(WAIT).unwrap();
        // The frame is held; every call returns without waiting for it.
        let begin = Instant::now();
        for n in 0..1000 {
            viewport.set_camera(view(12.0 + f64::from(n))).unwrap();
        }
        assert_eq!(viewport.frames(), 1);
        assert!(viewport.latest_frame().is_some());
        assert!(viewport.dirty().camera);
        assert!(begin.elapsed() < Duration::from_secs(10));
        release.send(()).unwrap();
        started.recv_timeout(WAIT).unwrap();
        release.send(()).unwrap();
        viewport.sync(WAIT).unwrap();
        // The held frame and one frame for the latest camera.
        assert_eq!(viewport.frames(), 3);
        assert_eq!(lock(&viewport.shared).inputs.frame, view(1011.0));
    }

    #[test]
    fn a_job_sees_the_settings_of_the_frame_it_runs_with() {
        let clock = ManualClock::default();
        let viewport = viewport_with_clock(&clock);
        let budget = |v: &Viewport| {
            v.with_scene(WAIT, |_, _, settings| settings.lod_budget_px)
                .unwrap()
        };
        assert_eq!(budget(&viewport), RenderSettings::DEFAULT.lod_budget_px);
        // The camera moves and the clock stays: the frame is drawn at navigation quality, and a
        // pick run with it tests the same levels of detail.
        viewport.set_camera(view(11.0)).unwrap();
        assert_eq!(
            budget(&viewport),
            RenderSettings::DEFAULT.lod_budget_px * NAVIGATION_LOD_FACTOR
        );
        clock.advance(SETTLE_TIME);
        assert_eq!(budget(&viewport), RenderSettings::DEFAULT.lod_budget_px);
    }

    #[test]
    fn thread_errors_surface_on_the_next_call() {
        let viewport = viewport(false);
        let mut bad = line(1, 1, 0.0);
        if let Outcome::Items(x) = &mut bad.outcome
            && let RenderItem::Lines(l) = &mut x.items[0]
        {
            l.origin.x = f64::NAN;
        }
        viewport.update_scene(vec![bad]).unwrap();
        assert_eq!(
            viewport.sync(WAIT).unwrap_err().code(),
            "gpu.value.not_finite"
        );
        assert_eq!(viewport.frames(), 1);
        viewport.set_camera(view(13.0)).unwrap();
        viewport.sync(WAIT).unwrap();
    }

    #[test]
    fn device_errors_surface_as_codes_and_drop_stays_bounded() {
        let gpu = gpu();
        let mut config = ViewportConfig::new(16, 16, view(10.0));
        config.settle = NEVER;
        let viewport = Viewport::new(&gpu, config).unwrap();
        viewport.sync(WAIT).unwrap();
        // On the pinned adapter a destroyed device reports validation errors; a backend that
        // reports the loss stops the viewport with `gpu.device.lost`.
        gpu.device().destroy();
        let device_error = |e: Error| ["gpu.validation", "gpu.device.lost"].contains(&e.code());
        let _ = viewport.set_camera(view(20.0));
        assert!(device_error(viewport.sync(WAIT).unwrap_err()));
        let _ = viewport.set_camera(view(21.0));
        assert!(device_error(viewport.sync(WAIT).unwrap_err()));
        assert_eq!(viewport.frames(), 1);
        let shared = Arc::downgrade(&viewport.shared);
        drop(viewport);
        assert!(shared.upgrade().is_none());
    }

    #[test]
    fn dropping_joins_the_thread() {
        let viewport = viewport(false);
        let shared = Arc::downgrade(&viewport.shared);
        drop(viewport);
        assert!(shared.upgrade().is_none());
    }

    #[test]
    fn dropping_during_a_slow_frame_waits_a_bounded_time() {
        let viewport = viewport(false);
        let (started, release) = hold(&viewport);
        lock(&viewport.shared).join_timeout = Duration::from_millis(50);
        viewport.set_camera(view(11.0)).unwrap();
        started.recv_timeout(WAIT).unwrap();
        let shared = Arc::clone(&viewport.shared);
        let begin = Instant::now();
        drop(viewport);
        assert!(begin.elapsed() < Duration::from_secs(10));
        assert!(!lock(&shared).exited);
        // The detached thread exits after its frame.
        release.send(()).unwrap();
        let state = shared
            .done
            .wait_timeout_while(lock(&shared), WAIT, |s| !s.exited)
            .unwrap()
            .0;
        assert!(state.exited);
    }

    #[test]
    fn a_settled_camera_draws_one_frame_at_full_quality() {
        let clock = ManualClock::default();
        let viewport = viewport_with_clock(&clock);
        let before = viewport.frames();
        viewport.set_camera(view(12.0)).unwrap();
        let moved = viewport.wait_frame(before, WAIT).unwrap();
        assert!(moved.reasons.camera);
        assert!(
            !moved.reasons.quality,
            "navigation draws at the camera's own reason"
        );
        // Without further input the thread draws again once the camera has been still.
        clock.advance(SETTLE_TIME);
        viewport.wake();
        let settled = viewport.wait_frame(moved.number, WAIT).unwrap();
        assert!(settled.reasons.quality);
        assert!(!settled.reasons.camera);
        assert!(!viewport.dirty().any());
    }
}
