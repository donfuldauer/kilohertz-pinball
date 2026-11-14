//! Flat-colour PBR materials: one per physics [`Material`], one per insert
//! colour.

use super::glb::FlatMaterial;
use pin_core::Material;

/// The flat-colour material of a physics material. Its name is the
/// material's TOML name, so the runtime can map a collider back to its
/// contact coefficients.
pub fn physical(m: Material) -> FlatMaterial {
    // (linear base colour RGBA, metallic, roughness)
    let (base_color, metallic, roughness) = match m {
        Material::Wood => ([0.58, 0.37, 0.18, 1.0], 0.0, 0.55),
        Material::Rubber => ([0.02, 0.02, 0.02, 1.0], 0.0, 0.9),
        Material::Steel => ([0.62, 0.62, 0.64, 1.0], 1.0, 0.25),
        Material::Plastic => ([0.8, 0.8, 0.8, 1.0], 0.0, 0.4),
        Material::Acrylic => ([0.85, 0.92, 1.0, 0.3], 0.0, 0.05),
        Material::Ceramic => ([0.9, 0.9, 0.88, 1.0], 0.0, 0.2),
        Material::Resin => ([0.45, 0.2, 0.06, 1.0], 0.0, 0.35),
    };
    FlatMaterial {
        name: m.name().to_owned(),
        base_color,
        metallic,
        roughness,
    }
}

/// sRGB channel (0..=255) to linear.
fn srgb_to_linear(c: u8) -> f32 {
    let c = f64::from(c) / 255.0;
    let l = if c <= 0.04045 {
        c / 12.92
    } else {
        libm::pow((c + 0.055) / 1.055, 2.4)
    };
    l as f32
}

/// Parses `#rrggbb`.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Insert material of colour `rgb` (sRGB), named `insert #rrggbb`.
pub fn insert(rgb: [u8; 3]) -> FlatMaterial {
    FlatMaterial {
        name: format!("insert #{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]),
        base_color: [
            srgb_to_linear(rgb[0]),
            srgb_to_linear(rgb[1]),
            srgb_to_linear(rgb[2]),
            1.0,
        ],
        metallic: 0.0,
        roughness: 0.3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_material_has_a_named_colour() {
        for m in Material::ALL {
            let f = physical(m);
            assert_eq!(f.name, m.name());
            assert!(f.base_color.iter().all(|c| (0.0..=1.0).contains(c)));
        }
    }

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex("#ff7a00"), Some([255, 122, 0]));
        assert_eq!(parse_hex("ff7a00"), None);
        assert_eq!(parse_hex("#ff7a0"), None);
        assert_eq!(parse_hex("#gg7a00"), None);
        assert_eq!(parse_hex("#ff7aé0"), None);
        let m = insert([255, 0, 0]);
        assert_eq!(m.name, "insert #ff0000");
        assert_eq!(m.base_color[0], 1.0);
        assert_eq!(m.base_color[1], 0.0);
        assert!((srgb_to_linear(128) - 0.2158).abs() < 1e-3);
    }
}
