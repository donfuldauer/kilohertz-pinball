//! Path selection: ray queries or probes, and the automatic fallback.

use super::{RAY_QUERY_FEATURE, RayQueryConfig, RayQueryReflections, ReflectionQuality};
use crate::gpu::GpuContext;
use crate::pbr::{MeshGeometry, MeshId, PbrRenderer};
use crate::reflections::{EnvironmentReflections, OctahedralProbes, ReflectionSource};
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

/// What a device can do for the ray-query path. Implemented by
/// [`GpuContext`] (its device's features) and by [`RayCaps`] (a plain
/// value, so the fallback logic is testable without a GPU).
pub trait RayCapability {
    /// `true` when the device enabled [`RAY_QUERY_FEATURE`].
    fn supports_ray_query(&self) -> bool;

    /// `true` when the device is known to be lost (default: `false`).
    /// wgpu 30 reports a loss only through
    /// `wgpu::Device::set_device_lost_callback` (there is no query); every
    /// [`GpuContext`] registers that callback when it is created, and its
    /// implementation reports the real flag ([`GpuContext::device_lost`]),
    /// so [`select_reflection_source`] picks the probes on a lost device
    /// (SPEC §4 Subphase 5.4 item 4). A [`RayQueryReflections`] built on
    /// the context shares the same flag: after a loss its
    /// [`prepare`](crate::reflections::ReflectionSource::prepare) falls
    /// back for good and returns the loss to the caller
    /// ([`RayQueryReflections::device_lost`]).
    fn device_lost(&self) -> bool {
        false
    }
}

impl RayCapability for GpuContext {
    fn supports_ray_query(&self) -> bool {
        self.device().features().contains(RAY_QUERY_FEATURE)
    }

    fn device_lost(&self) -> bool {
        GpuContext::device_lost(self)
    }
}

/// A fixed capability (tests, and callers that know better than the
/// device).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RayCaps {
    /// The device has [`RAY_QUERY_FEATURE`].
    pub ray_query: bool,
    /// The device is lost.
    pub device_lost: bool,
}

impl RayCapability for RayCaps {
    fn supports_ray_query(&self) -> bool {
        self.ray_query
    }

    fn device_lost(&self) -> bool {
        self.device_lost
    }
}

/// Why the probe path is used.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FallbackReason {
    /// [`ReflectionQuality::Off`] (the default, D33).
    QualityOff,
    /// The device lacks [`RAY_QUERY_FEATURE`].
    FeatureAbsent,
    /// The device is lost.
    DeviceLost,
    /// Ray-query setup or a frame failed (shader, pipeline or
    /// acceleration-structure error); permanent for that source.
    RuntimeError,
}

impl fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FallbackReason::QualityOff => "reflection quality is off",
            FallbackReason::FeatureAbsent => "the device lacks Features::EXPERIMENTAL_RAY_QUERY",
            FallbackReason::DeviceLost => "the device is lost",
            FallbackReason::RuntimeError => "a ray-query error at runtime",
        })
    }
}

/// The reflection path [`select_reflection_source`] takes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReflectionPath {
    /// [`RayQueryReflections`].
    RayQuery,
    /// [`OctahedralProbes`], for this reason.
    Probes(FallbackReason),
}

/// The path for `quality` on a device with `caps` (pure; checked in this
/// order: quality off, device lost, feature absent).
pub fn choose_reflection_path(
    quality: ReflectionQuality,
    caps: &dyn RayCapability,
) -> ReflectionPath {
    if quality == ReflectionQuality::Off {
        ReflectionPath::Probes(FallbackReason::QualityOff)
    } else if caps.device_lost() {
        ReflectionPath::Probes(FallbackReason::DeviceLost)
    } else if !caps.supports_ray_query() {
        ReflectionPath::Probes(FallbackReason::FeatureAbsent)
    } else {
        ReflectionPath::RayQuery
    }
}

static FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

/// Prints the fallback note once per process (later fallbacks are
/// silent); `true` when this call printed it.
pub(crate) fn log_fallback_once(what: &str) -> bool {
    if FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
        return false;
    }
    eprintln!("pin_render: ray-query reflections unavailable ({what}); using reflection probes");
    true
}

/// The reflection source for `quality` with automatic fallback (never
/// panics):
///
/// * [`ReflectionQuality::Off`] → [`OctahedralProbes`] (D33 default).
/// * Feature absent or device lost → [`OctahedralProbes`] (logged once).
/// * Otherwise [`RayQueryReflections`] over the static `meshes` (one BLAS
///   each); if its setup fails → [`OctahedralProbes`] (logged once). At
///   runtime it falls back to its own probes permanently on any
///   acceleration-structure or device error.
///
/// If even the probes cannot be created, the probe-less
/// [`EnvironmentReflections`] is returned (logged).
pub fn select_reflection_source(
    ctx: &GpuContext,
    quality: ReflectionQuality,
    pbr: &PbrRenderer,
    meshes: &[(MeshId, &MeshGeometry)],
    config: &RayQueryConfig,
) -> Box<dyn ReflectionSource> {
    select_reflection_source_with(ctx, ctx, quality, pbr, meshes, config)
}

/// [`select_reflection_source`] with the capability given by `caps`
/// instead of the device (a fake [`RayCaps`] forces a path).
pub fn select_reflection_source_with(
    caps: &dyn RayCapability,
    ctx: &GpuContext,
    quality: ReflectionQuality,
    pbr: &PbrRenderer,
    meshes: &[(MeshId, &MeshGeometry)],
    config: &RayQueryConfig,
) -> Box<dyn ReflectionSource> {
    match choose_reflection_path(quality, caps) {
        ReflectionPath::RayQuery => {
            match RayQueryReflections::new(ctx, pbr, quality, meshes, config) {
                Ok(rq) => return Box::new(rq),
                Err(e) => {
                    log_fallback_once(&e.to_string());
                }
            }
        }
        ReflectionPath::Probes(FallbackReason::QualityOff) => {}
        ReflectionPath::Probes(reason) => {
            log_fallback_once(&reason.to_string());
        }
    }
    match OctahedralProbes::new(ctx, pbr, config.probes) {
        Ok(p) => Box::new(p),
        Err(e) => {
            eprintln!("pin_render: reflection probes unavailable ({e}); using the environment");
            Box::new(EnvironmentReflections::new(ctx, pbr))
        }
    }
}
