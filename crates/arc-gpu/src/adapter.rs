//! Adapter selection and the device (CORE-GPU-001).
//!
//! # Selection rules
//!
//! 1. Adapters are enumerated on the Vulkan, Metal, DirectX 12 and OpenGL backends, or only on
//!    the backend [`GpuSettings::backend`] forces. [`GpuSettings::adapter_name`] keeps only the
//!    adapters whose name contains it, ignoring case.
//! 2. An adapter is *suitable* when it supports the downlevel default limits and drawing with a
//!    base vertex. Unsuitable adapters are skipped.
//! 3. Suitable adapters are ranked in three tiers: hardware adapters on Vulkan, Metal or
//!    DirectX 12; software adapters on those backends; adapters on the OpenGL backend (hardware
//!    before software). Within a tier, discrete adapters come before integrated, virtual and
//!    other ones, then backends in the order Vulkan, Metal, DirectX 12. The first adapter wins.
//! 4. The second and third tiers are *fallbacks*. With [`GpuSettings::allow_fallback`] false
//!    they are not used. A forced backend or name never falls back to another adapter: when
//!    nothing matches, creation fails with `gpu.adapter.not_found`.
//!
//! The [`AdapterReport`] says which adapter was chosen and which features are reduced on it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use arc_base::{Error, Result};

use crate::block::block_on;
use crate::error::codes;

/// How long adapter and device requests and error-scope results may take.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A graphics backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BackendKind {
    /// Vulkan.
    Vulkan,
    /// Metal.
    Metal,
    /// DirectX 12.
    Dx12,
    /// OpenGL or OpenGL ES, the fallback backend.
    Gl,
}

impl BackendKind {
    fn from_wgpu(backend: wgpu::Backend) -> Option<Self> {
        match backend {
            wgpu::Backend::Vulkan => Some(Self::Vulkan),
            wgpu::Backend::Metal => Some(Self::Metal),
            wgpu::Backend::Dx12 => Some(Self::Dx12),
            wgpu::Backend::Gl => Some(Self::Gl),
            _ => None,
        }
    }

    fn to_wgpu(self) -> wgpu::Backends {
        match self {
            Self::Vulkan => wgpu::Backends::VULKAN,
            Self::Metal => wgpu::Backends::METAL,
            Self::Dx12 => wgpu::Backends::DX12,
            Self::Gl => wgpu::Backends::GL,
        }
    }
}

/// The kind of device behind an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AdapterKind {
    /// A discrete GPU.
    Discrete,
    /// A GPU integrated with the CPU.
    Integrated,
    /// A virtualized GPU.
    Virtual,
    /// Any other device.
    Other,
    /// A software rasterizer running on the CPU.
    Software,
}

impl AdapterKind {
    fn from_wgpu(kind: wgpu::DeviceType) -> Self {
        match kind {
            wgpu::DeviceType::DiscreteGpu => Self::Discrete,
            wgpu::DeviceType::IntegratedGpu => Self::Integrated,
            wgpu::DeviceType::VirtualGpu => Self::Virtual,
            wgpu::DeviceType::Cpu => Self::Software,
            wgpu::DeviceType::Other => Self::Other,
        }
    }
}

/// A feature that is unavailable or limited on the chosen adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReducedFeature {
    /// The adapter is a software rasterizer: rendering is correct but slow.
    SoftwareRasterizer,
    /// Indirect multi-draw is unavailable; culling will use the CPU path.
    GpuCulling,
    /// Four-sample multisampling of the color and depth targets is unavailable; images render
    /// with one sample.
    Multisampling,
    /// The adapter supports only the downlevel default limits, not the full default limits.
    DownlevelLimits,
}

/// The settings of adapter selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuSettings {
    /// Use only this backend.
    pub backend: Option<BackendKind>,
    /// Use only adapters whose name contains this text, ignoring case.
    pub adapter_name: Option<String>,
    /// Never use a fallback adapter (a software adapter or the OpenGL backend). `false` is the
    /// default, so fallbacks are used when nothing better exists.
    pub forbid_fallback: bool,
}

impl GpuSettings {
    /// Whether fallback adapters may be used.
    #[must_use]
    pub fn allow_fallback(&self) -> bool {
        !self.forbid_fallback
    }
}

/// The adapter a [`Gpu`] runs on, for display and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterReport {
    /// The adapter name the driver reports.
    pub name: String,
    /// The driver name and version text, possibly empty.
    pub driver: String,
    /// The backend.
    pub backend: BackendKind,
    /// The kind of device.
    pub kind: AdapterKind,
    /// Whether the adapter is a software rasterizer.
    pub software: bool,
    /// Whether the adapter came from a fallback tier (see the [module documentation](self)).
    pub fallback: bool,
    /// The features that are unavailable or limited, in order.
    pub reduced: Vec<ReducedFeature>,
    /// The largest multisample count usable for rendering: 4 or 1.
    pub max_samples: u32,
}

/// The rank of an adapter: lower is better. `None` for a backend this crate does not use.
pub(crate) fn rank(backend: BackendKind, kind: AdapterKind) -> (u8, u8, u8) {
    let tier = match (backend, kind) {
        (BackendKind::Gl, AdapterKind::Software) => 3,
        (BackendKind::Gl, _) => 2,
        (_, AdapterKind::Software) => 1,
        _ => 0,
    };
    let kind_order = match kind {
        AdapterKind::Discrete => 0,
        AdapterKind::Integrated => 1,
        AdapterKind::Virtual => 2,
        AdapterKind::Other => 3,
        AdapterKind::Software => 4,
    };
    let backend_order = match backend {
        BackendKind::Vulkan => 0,
        BackendKind::Metal => 1,
        BackendKind::Dx12 => 2,
        BackendKind::Gl => 3,
    };
    (tier, kind_order, backend_order)
}

/// Errors captured outside error scopes, and the device-lost flag.
#[derive(Debug, Default)]
struct Health {
    lost: AtomicBool,
    uncaptured: Mutex<Option<(bool, String)>>,
}

/// A device and queue created from the selected adapter. Cloning shares the device.
#[derive(Debug, Clone)]
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    report: Arc<AdapterReport>,
    health: Arc<Health>,
}

/// The limits every device is created with.
fn required_limits() -> wgpu::Limits {
    wgpu::Limits::downlevel_defaults()
}

fn suitable(adapter: &wgpu::Adapter) -> bool {
    required_limits().check_limits(&adapter.limits())
        && adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::BASE_VERTEX)
}

fn max_samples(adapter: &wgpu::Adapter) -> u32 {
    let four = |format| {
        adapter
            .get_texture_format_features(format)
            .flags
            .sample_count_supported(4)
    };
    if four(crate::render::COLOR_FORMAT) && four(crate::render::DEPTH_FORMAT) {
        4
    } else {
        1
    }
}

impl Gpu {
    /// Selects an adapter by the [rules](self) and creates a device on it.
    ///
    /// # Errors
    ///
    /// `gpu.adapter.not_found` when no suitable adapter matches the settings;
    /// `gpu.device.request_failed` when the adapter refuses the device; `gpu.timeout` when the
    /// driver does not answer within 30 seconds.
    pub fn new(settings: &GpuSettings) -> Result<Self> {
        let backends = settings.backend.map_or(
            wgpu::Backends::VULKAN
                | wgpu::Backends::METAL
                | wgpu::Backends::DX12
                | wgpu::Backends::GL,
            BackendKind::to_wgpu,
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::from_build_config(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapters = block_on(instance.enumerate_adapters(backends), REQUEST_TIMEOUT)?;
        let wanted = settings.adapter_name.as_deref().map(str::to_lowercase);
        let mut ranked: Vec<_> = adapters
            .into_iter()
            .filter_map(|adapter| {
                let info = adapter.get_info();
                let backend = BackendKind::from_wgpu(info.backend)?;
                let kind = AdapterKind::from_wgpu(info.device_type);
                let rank = rank(backend, kind);
                let name_ok = wanted
                    .as_ref()
                    .is_none_or(|w| info.name.to_lowercase().contains(w.as_str()));
                (name_ok && (rank.0 == 0 || settings.allow_fallback()) && suitable(&adapter))
                    .then_some((rank, info.name.clone(), adapter))
            })
            .collect();
        // The name breaks ties so selection is deterministic.
        ranked.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        let Some((rank, _, adapter)) = ranked.into_iter().next() else {
            return Err(Error::new(codes::ADAPTER_NOT_FOUND));
        };
        Self::from_adapter(&adapter, rank.0 > 0)
    }

    fn from_adapter(adapter: &wgpu::Adapter, fallback: bool) -> Result<Self> {
        let info = adapter.get_info();
        let backend = BackendKind::from_wgpu(info.backend)
            .ok_or_else(|| Error::new(codes::ADAPTER_NOT_FOUND))?;
        let kind = AdapterKind::from_wgpu(info.device_type);
        let samples = max_samples(adapter);
        let mut reduced = Vec::new();
        if kind == AdapterKind::Software {
            reduced.push(ReducedFeature::SoftwareRasterizer);
        }
        if !adapter
            .features()
            .contains(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT)
        {
            reduced.push(ReducedFeature::GpuCulling);
        }
        if samples < 4 {
            reduced.push(ReducedFeature::Multisampling);
        }
        if !wgpu::Limits::default().check_limits(&adapter.limits()) {
            reduced.push(ReducedFeature::DownlevelLimits);
        }
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("arc-gpu device"),
            required_features: wgpu::Features::empty(),
            required_limits: required_limits(),
            ..wgpu::DeviceDescriptor::default()
        };
        let (device, queue) = block_on(adapter.request_device(&descriptor), REQUEST_TIMEOUT)?
            .map_err(|e| Error::new(codes::DEVICE_REQUEST_FAILED).with_source(e))?;
        let health = Arc::new(Health::default());
        let lost = Arc::clone(&health);
        device.set_device_lost_callback(move |_, _| lost.lost.store(true, Ordering::SeqCst));
        let captured = Arc::clone(&health);
        // Replaces the default handler, which panics; the error surfaces at the next check.
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            let oom = matches!(error, wgpu::Error::OutOfMemory { .. });
            let mut slot = captured
                .uncaptured
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            slot.get_or_insert((oom, error.to_string()));
        }));
        Ok(Self {
            device,
            queue,
            report: Arc::new(AdapterReport {
                name: info.name,
                driver: format!("{} {}", info.driver, info.driver_info)
                    .trim()
                    .to_owned(),
                backend,
                kind,
                software: kind == AdapterKind::Software,
                fallback,
                reduced,
                max_samples: samples,
            }),
            health,
        })
    }

    /// The adapter this device runs on.
    #[must_use]
    pub fn report(&self) -> &AdapterReport {
        &self.report
    }

    /// The `wgpu` device, for products that add their own passes.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The `wgpu` queue.
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Whether the device has been lost.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.health.lost.load(Ordering::SeqCst)
    }

    /// Fails with `gpu.device.lost` after device loss, or with the first error the device
    /// reported outside an error scope.
    pub(crate) fn check(&self) -> Result<()> {
        if self.is_lost() {
            return Err(Error::new(codes::DEVICE_LOST));
        }
        let taken = self
            .health
            .uncaptured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match taken {
            Some((true, text)) => Err(Error::new(codes::OUT_OF_MEMORY).with_detail("device", text)),
            Some((false, text)) => Err(Error::new(codes::VALIDATION).with_detail("device", text)),
            None => Ok(()),
        }
    }

    /// Runs `work` inside out-of-memory and validation error scopes, so a device error becomes
    /// `gpu.out_of_memory` or `gpu.validation` instead of being logged.
    pub(crate) fn scoped<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        self.check()?;
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = work();
        let validation = block_on(validation.pop(), REQUEST_TIMEOUT)?;
        let memory = block_on(memory.pop(), REQUEST_TIMEOUT)?;
        self.check()?;
        if let Some(error) = memory {
            return Err(Error::new(codes::OUT_OF_MEMORY).with_detail("device", error.to_string()));
        }
        if let Some(error) = validation {
            return Err(Error::new(codes::VALIDATION).with_detail("device", error.to_string()));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_native_adapters_rank_before_fallbacks() {
        use AdapterKind::{Discrete, Integrated, Software};
        use BackendKind::{Dx12, Gl, Metal, Vulkan};
        let mut adapters = vec![
            (Gl, Discrete),
            (Vulkan, Software),
            (Dx12, Integrated),
            (Gl, Software),
            (Dx12, Discrete),
            (Vulkan, Discrete),
            (Metal, Integrated),
        ];
        adapters.sort_by_key(|(b, k)| rank(*b, *k));
        assert_eq!(
            adapters,
            vec![
                (Vulkan, Discrete),
                (Dx12, Discrete),
                (Metal, Integrated),
                (Dx12, Integrated),
                (Vulkan, Software),
                (Gl, Discrete),
                (Gl, Software),
            ]
        );
    }

    #[test]
    fn only_native_hardware_is_not_a_fallback() {
        assert_eq!(rank(BackendKind::Metal, AdapterKind::Virtual).0, 0);
        assert!(rank(BackendKind::Dx12, AdapterKind::Software).0 > 0);
        assert!(rank(BackendKind::Gl, AdapterKind::Integrated).0 > 0);
    }
}
