//! Plan-view polygon helpers (f64, any length unit).

/// Twice the signed area (positive counter-clockwise).
pub fn signed_area2(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// Whether the closed segments `a b` and `c d` share a point.
pub fn segments_touch(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let (d1, d2) = (orient(c, d, a), orient(c, d, b));
    let (d3, d4) = (orient(a, b, c), orient(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && on_segment(c, d, a))
        || (d2 == 0.0 && on_segment(c, d, b))
        || (d3 == 0.0 && on_segment(a, b, c))
        || (d4 == 0.0 && on_segment(a, b, d))
}

/// The first pair of edges `(i, j)` (edge `i` runs from point `i` to point
/// `i + 1`, cyclically) of the closed polygon `p` that touch although they
/// are not neighbours, or that fold back onto each other although they
/// are; `None` for a simple polygon.
pub fn self_intersection(p: &[[f64; 2]]) -> Option<(usize, usize)> {
    let n = p.len();
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        for j in i + 1..n {
            let (c, d) = (p[j], p[(j + 1) % n]);
            let neighbours = j == i + 1 || (i == 0 && j == n - 1);
            if neighbours {
                // Neighbours share one end; they must not overlap beyond it.
                let (shared, other_a, other_b) = if j == i + 1 { (b, a, d) } else { (a, b, c) };
                let collinear = orient(other_a, shared, other_b) == 0.0;
                let dot = (other_a[0] - shared[0]) * (other_b[0] - shared[0])
                    + (other_a[1] - shared[1]) * (other_b[1] - shared[1]);
                if n == 3 && collinear || collinear && dot > 0.0 {
                    return Some((i, j));
                }
            } else if segments_touch(a, b, c, d) {
                return Some((i, j));
            }
        }
    }
    None
}

/// Whether `p` lies inside (or on) the closed polygon `poly`.
pub fn contains(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if orient(a, b, p) == 0.0 && on_segment(a, b, p) {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Ear-clipping triangulation of a simple polygon given counter-clockwise;
/// triangles are counter-clockwise (normal +Z). `None` if no ear is found
/// (not a simple counter-clockwise polygon).
pub fn triangulate(p: &[[f64; 2]]) -> Option<Vec<[usize; 3]>> {
    let mut idx: Vec<usize> = (0..p.len()).collect();
    let mut out = Vec::with_capacity(p.len().saturating_sub(2));
    while idx.len() > 3 {
        let m = idx.len();
        let ear = (0..m).find(|&k| {
            let (a, b, c) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            if orient(p[a], p[b], p[c]) <= 0.0 {
                return false;
            }
            idx.iter().all(|&q| {
                q == a
                    || q == b
                    || q == c
                    || !(orient(p[a], p[b], p[q]) >= 0.0
                        && orient(p[b], p[c], p[q]) >= 0.0
                        && orient(p[c], p[a], p[q]) >= 0.0)
            })
        });
        match ear {
            Some(ear) => {
                out.push([idx[(ear + m - 1) % m], idx[ear], idx[(ear + 1) % m]]);
                idx.remove(ear);
            }
            None => {
                // A vertex on the straight line between its neighbours adds
                // no area: drop it and look again.
                let flat = (0..m).find(|&k| {
                    let (a, b, c) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
                    orient(p[a], p[b], p[c]) == 0.0
                })?;
                idx.remove(flat);
            }
        }
    }
    if idx.len() == 3 {
        if orient(p[idx[0]], p[idx[1]], p[idx[2]]) <= 0.0 {
            return None;
        }
        out.push([idx[0], idx[1], idx[2]]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

    #[test]
    fn area_and_orientation() {
        assert_eq!(signed_area2(&SQUARE), 2.0);
        let mut cw = SQUARE;
        cw.reverse();
        assert_eq!(signed_area2(&cw), -2.0);
    }

    #[test]
    fn simple_and_crossing_polygons() {
        assert_eq!(self_intersection(&SQUARE), None);
        let bow = [[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]];
        assert_eq!(self_intersection(&bow), Some((0, 2)));
        // A spike folding back along its own edge.
        let spike = [[0.0, 0.0], [2.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        assert!(self_intersection(&spike).is_some());
        // Degenerate triangle.
        assert!(self_intersection(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]).is_some());
        // Touching at a vertex (figure of two triangles).
        let touch = [[0.0, 0.0], [1.0, 1.0], [2.0, 0.0], [2.0, 2.0], [1.0, 1.0], [0.0, 2.0]];
        assert!(self_intersection(&touch).is_some());
    }

    #[test]
    fn point_in_polygon() {
        assert!(contains(&SQUARE, [0.5, 0.5]));
        assert!(contains(&SQUARE, [1.0, 0.5]));
        assert!(!contains(&SQUARE, [1.5, 0.5]));
    }

    #[test]
    fn triangulates_concave_polygons() {
        let l = [
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let tris = triangulate(&l).unwrap();
        assert_eq!(tris.len(), 4);
        let area: f64 = tris
            .iter()
            .map(|t| orient(l[t[0]], l[t[1]], l[t[2]]))
            .sum();
        assert!((area - signed_area2(&l)).abs() < 1e-12);
        let mut cw = SQUARE;
        cw.reverse();
        assert!(triangulate(&cw).is_none());
        assert!(triangulate(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]).is_none());
    }
}
