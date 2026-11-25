//! Frame latency and software-adapter lean start (SPEC §7.5 REQ-LAT-006,
//! §2.2, D19).
//!
//! The playfield surface queues at most one frame ahead of the display; the
//! secondary surfaces keep the default of two. On a CPU (software) adapter the
//! app warns once and starts with [`LeanQuality::lean`] settings, which the
//! Low quality preset takes over later.

use crate::post::{AaMode, PostConfig};
use crate::topology::SurfaceRole;
use wgpu::DeviceType;

/// `desired_maximum_frame_latency` of the playfield surface (REQ-LAT-006).
pub const PLAYFIELD_FRAME_LATENCY: u32 = 1;

/// `desired_maximum_frame_latency` of the backglass and score surfaces.
pub const SECONDARY_FRAME_LATENCY: u32 = 2;

/// The `desired_maximum_frame_latency` a surface of `role` is configured with.
pub fn frame_latency_for(role: SurfaceRole) -> u32 {
    match role {
        SurfaceRole::Playfield => PLAYFIELD_FRAME_LATENCY,
        SurfaceRole::Backglass | SurfaceRole::Score => SECONDARY_FRAME_LATENCY,
    }
}

/// `true` when `info` describes a CPU (software) adapter.
pub fn is_cpu_adapter(info: &wgpu::AdapterInfo) -> bool {
    info.device_type == DeviceType::Cpu
}

/// Reduced quality settings used on a software adapter.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LeanQuality {
    /// MSAA sample count (1 = off).
    pub msaa_samples: u32,
    /// Whether bloom runs.
    pub bloom: bool,
    /// Whether motion blur runs.
    pub motion_blur: bool,
    /// Whether reflection probe captures run.
    pub probe_captures: bool,
    /// Render resolution scale relative to the surface.
    pub render_scale: f32,
}

impl LeanQuality {
    /// The lean settings: no MSAA, no bloom, no motion blur, no probe
    /// captures, half render scale.
    pub const fn lean() -> LeanQuality {
        LeanQuality {
            msaa_samples: 1,
            bloom: false,
            motion_blur: false,
            probe_captures: false,
            render_scale: 0.5,
        }
    }

    /// `post` with AA, bloom and motion blur switched off where these
    /// settings say so; every other field is unchanged.
    pub fn apply_to_post(&self, post: PostConfig) -> PostConfig {
        let mut out = post;
        if self.msaa_samples <= 1 {
            out.aa = AaMode::Off;
        }
        if !self.bloom {
            out.bloom.enabled = false;
        }
        if !self.motion_blur {
            out.motion_blur.enabled = false;
        }
        out
    }
}

/// Lean settings for `info`: on a CPU adapter, calls `warn` exactly once
/// (naming the adapter and what starts reduced) and returns
/// `Some(LeanQuality::lean())`; on any other adapter returns `None` and never
/// calls `warn`.
pub fn lean_quality_for(
    info: &wgpu::AdapterInfo,
    warn: &mut dyn FnMut(&str),
) -> Option<LeanQuality> {
    if !is_cpu_adapter(info) {
        return None;
    }
    let q = LeanQuality::lean();
    warn(&format!(
        "software graphics adapter \"{}\": starting with lean quality \
         (MSAA off, bloom off, motion blur off, probe captures off, render scale {})",
        info.name, q.render_scale
    ));
    Some(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(device_type: DeviceType) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: "test-adapter".to_string(),
            vendor: 0,
            device: 0,
            device_type,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: None,
            limit_bucket: None,
        }
    }

    const TYPES: [DeviceType; 5] = [
        DeviceType::Other,
        DeviceType::IntegratedGpu,
        DeviceType::DiscreteGpu,
        DeviceType::VirtualGpu,
        DeviceType::Cpu,
    ];

    /// REQ-LAT-006: playfield latency 1, secondary 2.
    #[test]
    fn req_lat_006_frame_latency_per_role() {
        assert_eq!(frame_latency_for(SurfaceRole::Playfield), 1);
        assert_eq!(frame_latency_for(SurfaceRole::Backglass), 2);
        assert_eq!(frame_latency_for(SurfaceRole::Score), 2);
        assert_eq!(PLAYFIELD_FRAME_LATENCY, 1);
        assert_eq!(SECONDARY_FRAME_LATENCY, 2);
    }

    /// REQ-LAT-006: only `DeviceType::Cpu` is a software adapter.
    #[test]
    fn req_lat_006_is_cpu_adapter() {
        for t in TYPES {
            assert_eq!(is_cpu_adapter(&info(t)), t == DeviceType::Cpu, "{t:?}");
        }
    }

    /// REQ-LAT-006: the lean values.
    #[test]
    fn req_lat_006_lean_values() {
        let q = LeanQuality::lean();
        assert_eq!(q.msaa_samples, 1);
        assert!(!q.bloom && !q.motion_blur && !q.probe_captures);
        assert_eq!(q.render_scale, 0.5);
    }

    /// REQ-LAT-006: a CPU adapter warns exactly once and gets lean settings.
    #[test]
    fn req_lat_006_cpu_adapter_warns_once() {
        let mut msgs: Vec<String> = Vec::new();
        let q = lean_quality_for(&info(DeviceType::Cpu), &mut |m| msgs.push(m.to_string()));
        assert_eq!(q, Some(LeanQuality::lean()));
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("test-adapter"), "{}", msgs[0]);
        assert!(msgs[0].contains("0.5"), "{}", msgs[0]);
        assert!(msgs[0].contains("probe"), "{}", msgs[0]);
    }

    /// REQ-LAT-006: other adapters get no lean settings and no warning; and
    /// repeated calls accumulate no state.
    #[test]
    fn req_lat_006_other_adapters_silent_and_stateless() {
        for t in TYPES.into_iter().filter(|t| *t != DeviceType::Cpu) {
            let mut n = 0;
            assert_eq!(lean_quality_for(&info(t), &mut |_| n += 1), None);
            assert_eq!(n, 0, "{t:?}");
        }
        for _ in 0..3 {
            let mut n = 0;
            assert!(lean_quality_for(&info(DeviceType::Cpu), &mut |_| n += 1).is_some());
            assert_eq!(n, 1);
        }
    }

    /// REQ-LAT-006: lean post turns AA, bloom and motion blur off and keeps
    /// everything else.
    #[test]
    fn req_lat_006_apply_to_post() {
        let mut post = PostConfig {
            exposure: 1.7,
            ..PostConfig::default()
        };
        post.bloom.enabled = true;
        post.motion_blur.enabled = true;
        let out = LeanQuality::lean().apply_to_post(post);
        assert_eq!(out.aa, AaMode::Off);
        assert!(!out.bloom.enabled && !out.motion_blur.enabled);
        assert_eq!(out.exposure, 1.7);
        assert_eq!(out.tone_map, post.tone_map);
        let mut want = post;
        want.aa = AaMode::Off;
        want.bloom.enabled = false;
        want.motion_blur.enabled = false;
        assert_eq!(out, want);
        // Settings that keep a feature leave it alone.
        let keep = LeanQuality {
            msaa_samples: 4,
            bloom: true,
            motion_blur: true,
            ..LeanQuality::lean()
        };
        assert_eq!(keep.apply_to_post(post), post);
    }
}
