//! The pinball material set and its parameter table.
//!
//! Every material is one parameter set of a single shading model (see
//! [`MaterialParams`]); the shader never branches on a material name.

/// The pinball materials.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MaterialKind {
    /// `pbr_chrome_steel`: polished steel ball (metal, near-mirror).
    ChromeSteel,
    /// `pbr_ceramic_pearl`: white ceramic ball (glossy dielectric with a
    /// soft clear coat).
    CeramicPearl,
    /// `pbr_uv_emissive`: fluorescent ball that glows with the GI tint.
    UvEmissive,
    /// `pbr_playfield_wood`: printed wood under a lacquer clear coat.
    PlayfieldWood,
    /// `pbr_light_insert`: translucent insert plastic lit from below
    /// (wrapped diffuse plus lamp-driven emission).
    LightInsert,
    /// `pbr_clear_acrylic`: clear plastic, drawn blended after the opaque
    /// pass.
    ClearAcrylic,
}

impl MaterialKind {
    /// Every material, in declaration order.
    pub const ALL: [MaterialKind; 6] = [
        MaterialKind::ChromeSteel,
        MaterialKind::CeramicPearl,
        MaterialKind::UvEmissive,
        MaterialKind::PlayfieldWood,
        MaterialKind::LightInsert,
        MaterialKind::ClearAcrylic,
    ];

    /// The material for a shader name (`pbr_chrome_steel`, … as in ball
    /// profiles and glTF material names); `None` for any other string
    /// (matching is exact).
    pub fn from_shader_name(name: &str) -> Option<MaterialKind> {
        MaterialKind::ALL
            .into_iter()
            .find(|k| k.shader_name() == name)
    }

    /// The shader name.
    pub const fn shader_name(self) -> &'static str {
        match self {
            MaterialKind::ChromeSteel => "pbr_chrome_steel",
            MaterialKind::CeramicPearl => "pbr_ceramic_pearl",
            MaterialKind::UvEmissive => "pbr_uv_emissive",
            MaterialKind::PlayfieldWood => "pbr_playfield_wood",
            MaterialKind::LightInsert => "pbr_light_insert",
            MaterialKind::ClearAcrylic => "pbr_clear_acrylic",
        }
    }

    /// `true` for materials drawn in the blended (transparent) pass.
    pub const fn is_transparent(self) -> bool {
        matches!(self, MaterialKind::ClearAcrylic)
    }

    /// The default parameters of this material.
    pub const fn params(self) -> MaterialParams {
        let p = MaterialParams::DEFAULT;
        match self {
            MaterialKind::ChromeSteel => MaterialParams {
                base_color: [0.95, 0.94, 0.92, 1.0],
                metallic: 1.0,
                roughness: 0.06,
                ..p
            },
            MaterialKind::CeramicPearl => MaterialParams {
                base_color: [0.86, 0.85, 0.82, 1.0],
                roughness: 0.4,
                clearcoat: 0.6,
                clearcoat_roughness: 0.18,
                ..p
            },
            MaterialKind::UvEmissive => MaterialParams {
                base_color: [0.02, 0.02, 0.025, 1.0],
                roughness: 0.45,
                gi_emissive: 1.5,
                ..p
            },
            MaterialKind::PlayfieldWood => MaterialParams {
                base_color: [0.42, 0.25, 0.12, 1.0],
                roughness: 0.75,
                clearcoat: 1.0,
                clearcoat_roughness: 0.06,
                ..p
            },
            MaterialKind::LightInsert => MaterialParams {
                base_color: [0.95, 0.75, 0.45, 1.0],
                roughness: 0.2,
                subsurface: 0.6,
                lamp_emissive: 4.0,
                ..p
            },
            MaterialKind::ClearAcrylic => MaterialParams {
                base_color: [0.97, 0.98, 1.0, 1.0],
                roughness: 0.05,
                transmission: 0.92,
                ..p
            },
        }
    }
}

/// Parameters of the shading model (metallic-roughness Cook-Torrance GGX +
/// Lambert, an optional clear-coat lobe, wrapped diffuse for subsurface
/// scattering, blended transmission and two emission terms). The layout is
/// the GPU layout (four `vec4<f32>`).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialParams {
    /// Linear RGB albedo / metal reflectance; alpha unused (1).
    pub base_color: [f32; 4],
    /// Constant emitted radiance (linear RGB, e.g. the glTF emissive
    /// factor).
    pub emissive: [f32; 3],
    /// Metalness `0..=1`.
    pub metallic: f32,
    /// Perceptual roughness `0..=1` (clamped to 0.03 in the shader).
    pub roughness: f32,
    /// Dielectric specular scale: F0 = 0.04 · `specular` (0 turns the
    /// specular lobe of a dielectric off: a pure Lambert material).
    pub specular: f32,
    /// Clear-coat layer strength `0..=1` (F0 0.04).
    pub clearcoat: f32,
    /// Clear-coat perceptual roughness.
    pub clearcoat_roughness: f32,
    /// Fraction of light transmitted through the surface `0..=1` (blended
    /// materials; the diffuse lobe is scaled by `1 − transmission`).
    pub transmission: f32,
    /// Diffuse wrap `0..=1`: light reaches past the terminator as in thin
    /// translucent plastic (0 = Lambert).
    pub subsurface: f32,
    /// Emission per unit of GI tint: adds `gi.rgb · gi.a · gi_emissive`.
    pub gi_emissive: f32,
    /// Emission at full lamp level: adds `base_color · lamp_emissive ·
    /// emission` (`emission` is the per-draw warmed-up lamp level).
    pub lamp_emissive: f32,
}

impl MaterialParams {
    /// A plain white dielectric (roughness 0.5, F0 0.04).
    pub const DEFAULT: MaterialParams = MaterialParams {
        base_color: [1.0, 1.0, 1.0, 1.0],
        emissive: [0.0; 3],
        metallic: 0.0,
        roughness: 0.5,
        specular: 1.0,
        clearcoat: 0.0,
        clearcoat_roughness: 0.1,
        transmission: 0.0,
        subsurface: 0.0,
        gi_emissive: 0.0,
        lamp_emissive: 0.0,
    };

    /// A pure Lambert material of albedo `albedo` (no specular lobe).
    pub const fn lambert(albedo: [f32; 3]) -> MaterialParams {
        MaterialParams {
            base_color: [albedo[0], albedo[1], albedo[2], 1.0],
            specular: 0.0,
            roughness: 1.0,
            ..MaterialParams::DEFAULT
        }
    }

    /// The parameters of `kind` with glTF metallic-roughness factors applied:
    /// `base_color` multiplies the material's albedo, `emissive` is added,
    /// and `metallic` / `roughness` replace the table values when given.
    pub fn from_gltf_factors(
        kind: MaterialKind,
        base_color: [f32; 4],
        metallic: Option<f32>,
        roughness: Option<f32>,
        emissive: [f32; 3],
    ) -> MaterialParams {
        let mut p = kind.params();
        for (c, f) in p.base_color.iter_mut().zip(base_color) {
            *c *= finite_or(f, 1.0).max(0.0);
        }
        for (e, f) in p.emissive.iter_mut().zip(emissive) {
            *e += finite_or(f, 0.0).max(0.0);
        }
        if let Some(m) = metallic {
            p.metallic = finite_or(m, p.metallic).clamp(0.0, 1.0);
        }
        if let Some(r) = roughness {
            p.roughness = finite_or(r, p.roughness).clamp(0.0, 1.0);
        }
        p
    }
}

impl Default for MaterialParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn finite_or(x: f32, or: f32) -> f32 {
    if x.is_finite() { x } else { or }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for k in MaterialKind::ALL {
            assert_eq!(MaterialKind::from_shader_name(k.shader_name()), Some(k));
        }
        assert_eq!(MaterialKind::from_shader_name("PBR_CHROME_STEEL"), None);
        assert_eq!(MaterialKind::from_shader_name(""), None);
        assert!(MaterialKind::ClearAcrylic.is_transparent());
        assert!(!MaterialKind::ChromeSteel.is_transparent());
        assert_eq!(core::mem::size_of::<MaterialParams>(), 64);
        assert_eq!(MaterialParams::default(), MaterialParams::DEFAULT);
    }

    #[test]
    fn gltf_factors() {
        let p = MaterialParams::from_gltf_factors(
            MaterialKind::PlayfieldWood,
            [0.5, f32::NAN, 1.0, 1.0],
            Some(2.0),
            Some(f32::NAN),
            [0.1, -1.0, 0.0],
        );
        let w = MaterialKind::PlayfieldWood.params();
        assert_eq!(p.base_color[0], w.base_color[0] * 0.5);
        assert_eq!(p.base_color[1], w.base_color[1]);
        assert_eq!(p.metallic, 1.0);
        assert_eq!(p.roughness, w.roughness);
        assert_eq!(p.emissive, [0.1, 0.0, 0.0]);
        let l = MaterialParams::lambert([0.5; 3]);
        assert_eq!(l.specular, 0.0);
    }
}
