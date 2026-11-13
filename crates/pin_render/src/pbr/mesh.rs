//! CPU meshes for the PBR renderer (positions, normals, triangle indices;
//! counter-clockwise front faces, as in glTF).

/// An indexed triangle mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshGeometry {
    /// Object-space positions.
    pub positions: Vec<[f32; 3]>,
    /// Object-space unit normals (one per position).
    pub normals: Vec<[f32; 3]>,
    /// Triangle list, counter-clockwise seen from outside.
    pub indices: Vec<u32>,
}

impl MeshGeometry {
    /// A UV sphere of `radius` around the origin with `segments` (≥ 3)
    /// around the z axis and `rings` (≥ 2) from pole to pole.
    pub fn uv_sphere(radius: f32, segments: u32, rings: u32) -> MeshGeometry {
        let segments = segments.max(3);
        let rings = rings.max(2);
        let mut m = MeshGeometry::default();
        for r in 0..=rings {
            let theta = core::f32::consts::PI * r as f32 / rings as f32;
            let (st, ct) = theta.sin_cos();
            for s in 0..=segments {
                let phi = core::f32::consts::TAU * s as f32 / segments as f32;
                let (sp, cp) = phi.sin_cos();
                let n = [st * cp, st * sp, ct];
                m.normals.push(n);
                m.positions
                    .push([n[0] * radius, n[1] * radius, n[2] * radius]);
            }
        }
        let row = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let a = r * row + s;
                let b = a + row;
                // Ring r is nearer +z; outward-facing CCW order.
                if r != 0 {
                    m.indices.extend_from_slice(&[a, b, a + 1]);
                }
                if r != rings - 1 {
                    m.indices.extend_from_slice(&[a + 1, b, b + 1]);
                }
            }
        }
        m
    }

    /// A square of side `size` in the z = 0 plane centred on the origin,
    /// facing +z.
    pub fn quad(size: f32) -> MeshGeometry {
        let h = 0.5 * size;
        MeshGeometry {
            positions: vec![[-h, -h, 0.0], [h, -h, 0.0], [h, h, 0.0], [-h, h, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_normal(m: &MeshGeometry, t: &[u32]) -> [f32; 3] {
        let p = |i: u32| m.positions[i as usize];
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    }

    #[test]
    fn sphere_faces_outward() {
        let m = MeshGeometry::uv_sphere(0.5, 12, 8);
        assert_eq!(m.positions.len(), m.normals.len());
        for t in m.indices.chunks(3) {
            let n = tri_normal(&m, t);
            let c: [f32; 3] = core::array::from_fn(|k| {
                t.iter().map(|&i| m.positions[i as usize][k]).sum::<f32>() / 3.0
            });
            let d = n[0] * c[0] + n[1] * c[1] + n[2] * c[2];
            assert!(d > 0.0, "inward triangle {t:?}");
        }
        let q = MeshGeometry::quad(2.0);
        assert!(tri_normal(&q, &q.indices[..3])[2] > 0.0);
        assert_eq!(MeshGeometry::uv_sphere(1.0, 0, 0).indices.len(), 3 * 3 * 2);
    }
}
