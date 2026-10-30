//! ROI region primitives and AABB (P3-F05). Pin: `Region.h` / `Region.cpp`.

use crate::math::{invalid, out_of_range};
use crate::{DomainError, Mat3, Vec3};

/// Axis-aligned bounds in train/world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn try_new(min: Vec3, max: Vec3) -> Result<Self, DomainError> {
        let a = min.to_array();
        let b = max.to_array();
        for d in 0..3 {
            if a[d] > b[d] {
                return Err(invalid(
                    "aabb",
                    format!("min[{d}]={} exceeds max[{d}]={}", a[d], b[d]),
                ));
            }
        }
        Ok(Self { min, max })
    }

    pub fn empty() -> Self {
        Self {
            min: Vec3::try_new(0.0, 0.0, 0.0).expect("zero"),
            max: Vec3::try_new(0.0, 0.0, 0.0).expect("zero"),
        }
    }

    pub fn from_points(points: &[[f64; 3]]) -> Result<Self, DomainError> {
        if points.is_empty() {
            return Ok(Self::empty());
        }
        let mut mn = points[0];
        let mut mx = points[0];
        for p in points.iter().skip(1) {
            for d in 0..3 {
                mn[d] = mn[d].min(p[d]);
                mx[d] = mx[d].max(p[d]);
            }
        }
        Self::try_new(
            Vec3::try_new(mn[0], mn[1], mn[2])?,
            Vec3::try_new(mx[0], mx[1], mx[2])?,
        )
    }

    pub fn contains(&self, p: Vec3) -> bool {
        let a = self.min.to_array();
        let b = self.max.to_array();
        let v = p.to_array();
        (0..3).all(|d| v[d] >= a[d] && v[d] <= b[d])
    }

    pub fn expand(&self, other: &Self) -> Result<Self, DomainError> {
        let a = self.min.to_array();
        let b = self.max.to_array();
        let c = other.min.to_array();
        let d = other.max.to_array();
        Self::try_new(
            Vec3::try_new(a[0].min(c[0]), a[1].min(c[1]), a[2].min(c[2]))?,
            Vec3::try_new(b[0].max(d[0]), b[1].max(d[1]), b[2].max(d[2]))?,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Box,
    Sphere,
    Ellipsoid,
    Cylinder,
    Halfspace,
    Csg,
}

impl RegionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Sphere => "sphere",
            Self::Ellipsoid => "ellipsoid",
            Self::Cylinder => "cylinder",
            Self::Halfspace => "halfspace",
            Self::Csg => "csg",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, DomainError> {
        match name {
            "box" => Ok(Self::Box),
            "sphere" => Ok(Self::Sphere),
            "ellipsoid" => Ok(Self::Ellipsoid),
            "cylinder" => Ok(Self::Cylinder),
            "halfspace" => Ok(Self::Halfspace),
            "union" | "intersection" | "difference" | "csg" => Ok(Self::Csg),
            _ => Err(invalid(
                "region.kind",
                format!("unknown region kind '{name}'"),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsgOp {
    Union,
    Intersection,
    Difference,
}

impl CsgOp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Intersection => "intersection",
            Self::Difference => "difference",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, DomainError> {
        match name {
            "union" => Ok(Self::Union),
            "intersection" => Ok(Self::Intersection),
            "difference" => Ok(Self::Difference),
            _ => Err(invalid("region.csg_op", format!("unknown CSG op '{name}'"))),
        }
    }
}

/// `q = R * (p - center)` — rows of `R` are the frame axes (`Region.cpp::to_frame`).
fn to_frame(center: Vec3, rotation: Mat3, p: Vec3) -> Result<Vec3, DomainError> {
    let d = p.try_sub(center)?;
    rotation.try_mul_vec(d)
}

fn require_nonneg_half(half: Vec3) -> Result<(), DomainError> {
    for (i, v) in half.to_array().iter().enumerate() {
        if *v < 0.0 || !v.is_finite() {
            return Err(out_of_range(
                "region.half",
                "non-negative finite",
                format!("[{i}]={v}"),
            ));
        }
    }
    Ok(())
}

/// Geometric payload for a non-CSG region.
#[derive(Clone, Debug, PartialEq)]
pub enum RegionGeom {
    /// Oriented box: `|R (p - c)|_k <= half_k`.
    Box {
        center: Vec3,
        half: Vec3,
        rotation: Mat3,
    },
    Sphere {
        center: Vec3,
        radius: f64,
    },
    Ellipsoid {
        center: Vec3,
        half: Vec3,
        rotation: Mat3,
    },
    /// Finite elliptical cylinder: `|q_z| <= half_z` and unit disk in xy / half_xy.
    Cylinder {
        center: Vec3,
        half: Vec3,
        rotation: Mat3,
    },
    /// Inside when `dot(normal, p) + offset >= 0` (C++ `HalfSpaceRegion`).
    Halfspace {
        normal: Vec3,
        offset: f64,
    },
}

/// Region tree node. CSG nodes own children (C++ may have N; we keep ≥1).
#[derive(Clone, Debug, PartialEq)]
pub enum Region {
    Leaf { kind: RegionKind, geom: RegionGeom },
    Csg { op: CsgOp, children: Vec<Region> },
}

impl Region {
    pub fn kind(&self) -> RegionKind {
        match self {
            Self::Leaf { kind, .. } => *kind,
            Self::Csg { .. } => RegionKind::Csg,
        }
    }

    pub fn box_region(center: Vec3, half: Vec3, rotation: Mat3) -> Result<Self, DomainError> {
        require_nonneg_half(half)?;
        let rotation = rotation.try_rotation()?;
        Ok(Self::Leaf {
            kind: RegionKind::Box,
            geom: RegionGeom::Box {
                center,
                half,
                rotation,
            },
        })
    }

    pub fn sphere(center: Vec3, radius: f64) -> Result<Self, DomainError> {
        if !(radius.is_finite() && radius >= 0.0) {
            return Err(out_of_range(
                "region.radius",
                "non-negative finite",
                format!("{radius}"),
            ));
        }
        Ok(Self::Leaf {
            kind: RegionKind::Sphere,
            geom: RegionGeom::Sphere { center, radius },
        })
    }

    pub fn ellipsoid(center: Vec3, half: Vec3, rotation: Mat3) -> Result<Self, DomainError> {
        require_nonneg_half(half)?;
        let rotation = rotation.try_rotation()?;
        Ok(Self::Leaf {
            kind: RegionKind::Ellipsoid,
            geom: RegionGeom::Ellipsoid {
                center,
                half,
                rotation,
            },
        })
    }

    pub fn cylinder(center: Vec3, half: Vec3, rotation: Mat3) -> Result<Self, DomainError> {
        require_nonneg_half(half)?;
        let rotation = rotation.try_rotation()?;
        Ok(Self::Leaf {
            kind: RegionKind::Cylinder,
            geom: RegionGeom::Cylinder {
                center,
                half,
                rotation,
            },
        })
    }

    pub fn halfspace(normal: Vec3, offset: f64) -> Result<Self, DomainError> {
        if !offset.is_finite() {
            return Err(out_of_range("region.offset", "finite", "nonfinite"));
        }
        if normal.length() == 0.0 {
            return Err(invalid(
                "region.normal",
                "halfspace normal must be non-zero",
            ));
        }
        Ok(Self::Leaf {
            kind: RegionKind::Halfspace,
            geom: RegionGeom::Halfspace { normal, offset },
        })
    }

    pub fn csg(op: CsgOp, children: Vec<Region>) -> Result<Self, DomainError> {
        if children.is_empty() {
            return Err(invalid("region.children", "CSG needs at least one child"));
        }
        Ok(Self::Csg { op, children })
    }

    fn leaf_contains(geom: &RegionGeom, p: Vec3) -> bool {
        match geom {
            RegionGeom::Box {
                center,
                half,
                rotation,
            } => {
                let Ok(q) = to_frame(*center, *rotation, p) else {
                    return false;
                };
                let he = half.to_array();
                let l = q.to_array();
                (0..3).all(|i| l[i].abs() <= he[i])
            }
            RegionGeom::Sphere { center, radius } => {
                let Ok(d) = p.try_sub(*center) else {
                    return false;
                };
                d.length() <= *radius
            }
            RegionGeom::Ellipsoid {
                center,
                half,
                rotation,
            } => {
                let Ok(q) = to_frame(*center, *rotation, p) else {
                    return false;
                };
                let he = half.to_array();
                let l = q.to_array();
                let mut s = 0.0;
                for i in 0..3 {
                    if he[i] == 0.0 {
                        if l[i] != 0.0 {
                            return false;
                        }
                    } else {
                        let t = l[i] / he[i];
                        s += t * t;
                    }
                }
                s <= 1.0
            }
            RegionGeom::Cylinder {
                center,
                half,
                rotation,
            } => {
                let Ok(q) = to_frame(*center, *rotation, p) else {
                    return false;
                };
                let he = half.to_array();
                if q.z().abs() > he[2] {
                    return false;
                }
                if he[0] == 0.0 || he[1] == 0.0 {
                    return q.x() == 0.0 && q.y() == 0.0;
                }
                let u = q.x() / he[0];
                let v = q.y() / he[1];
                u * u + v * v <= 1.0
            }
            RegionGeom::Halfspace { normal, offset } => normal.dot(p) + offset >= 0.0,
        }
    }

    pub fn contains(&self, p: Vec3) -> bool {
        match self {
            Self::Leaf { geom, .. } => Self::leaf_contains(geom, p),
            Self::Csg { op, children } => match op {
                CsgOp::Union => children.iter().any(|c| c.contains(p)),
                CsgOp::Intersection => children.iter().all(|c| c.contains(p)),
                CsgOp::Difference => {
                    let Some((first, rest)) = children.split_first() else {
                        return false;
                    };
                    first.contains(p) && rest.iter().all(|c| !c.contains(p))
                }
            },
        }
    }

    /// Conservative AABB (`None` for halfspace / unbounded).
    pub fn try_aabb(&self) -> Option<Aabb> {
        match self {
            Self::Leaf { geom, .. } => match geom {
                RegionGeom::Box {
                    center,
                    half,
                    rotation,
                }
                | RegionGeom::Cylinder {
                    center,
                    half,
                    rotation,
                } => {
                    // framed_bounds: e_j = sum_k |R[k,j]| * half[k]
                    let he = half.to_array();
                    let rm = rotation.row_major();
                    let mut lo = [0.0; 3];
                    let mut hi = [0.0; 3];
                    let c = center.to_array();
                    for j in 0..3 {
                        let mut e = 0.0;
                        for k in 0..3 {
                            e += rm[k * 3 + j].abs() * he[k];
                        }
                        lo[j] = c[j] - e;
                        hi[j] = c[j] + e;
                    }
                    Aabb::try_new(
                        Vec3::try_new(lo[0], lo[1], lo[2]).ok()?,
                        Vec3::try_new(hi[0], hi[1], hi[2]).ok()?,
                    )
                    .ok()
                }
                RegionGeom::Sphere { center, radius } => {
                    let c = center.to_array();
                    let r = *radius;
                    Aabb::try_new(
                        Vec3::try_new(c[0] - r, c[1] - r, c[2] - r).ok()?,
                        Vec3::try_new(c[0] + r, c[1] + r, c[2] + r).ok()?,
                    )
                    .ok()
                }
                RegionGeom::Ellipsoid {
                    center,
                    half,
                    rotation,
                } => {
                    // C++ ellipsoid AABB: e2_j = sum_k (R[k,j]*half[k])^2
                    let he = half.to_array();
                    let rm = rotation.row_major();
                    let c = center.to_array();
                    let mut lo = [0.0; 3];
                    let mut hi = [0.0; 3];
                    for j in 0..3 {
                        let mut e2 = 0.0;
                        for k in 0..3 {
                            let t = rm[k * 3 + j] * he[k];
                            e2 += t * t;
                        }
                        let e = e2.sqrt();
                        lo[j] = c[j] - e;
                        hi[j] = c[j] + e;
                    }
                    Aabb::try_new(
                        Vec3::try_new(lo[0], lo[1], lo[2]).ok()?,
                        Vec3::try_new(hi[0], hi[1], hi[2]).ok()?,
                    )
                    .ok()
                }
                RegionGeom::Halfspace { .. } => None,
            },
            Self::Csg { op, children } => {
                let mut bounds = Vec::new();
                for c in children {
                    bounds.push(c.try_aabb()?);
                }
                if bounds.is_empty() {
                    return None;
                }
                match op {
                    CsgOp::Union => {
                        let mut acc = bounds[0];
                        for b in bounds.iter().skip(1) {
                            acc = acc.expand(b).ok()?;
                        }
                        Some(acc)
                    }
                    CsgOp::Intersection => {
                        let mut mn = bounds[0].min.to_array();
                        let mut mx = bounds[0].max.to_array();
                        for b in bounds.iter().skip(1) {
                            let a = b.min.to_array();
                            let c = b.max.to_array();
                            for d in 0..3 {
                                mn[d] = mn[d].max(a[d]);
                                mx[d] = mx[d].min(c[d]);
                            }
                        }
                        if mn.iter().zip(mx.iter()).any(|(u, v)| u > v) {
                            Some(Aabb::empty())
                        } else {
                            Aabb::try_new(
                                Vec3::try_new(mn[0], mn[1], mn[2]).ok()?,
                                Vec3::try_new(mx[0], mx[1], mx[2]).ok()?,
                            )
                            .ok()
                        }
                    }
                    CsgOp::Difference => Some(bounds[0]),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_contains_and_aabb() {
        let r = Region::sphere(Vec3::try_new(0.0, 0.0, 0.0).unwrap(), 2.0).unwrap();
        assert!(r.contains(Vec3::try_new(1.0, 0.0, 0.0).unwrap()));
        assert!(!r.contains(Vec3::try_new(3.0, 0.0, 0.0).unwrap()));
        let aabb = r.try_aabb().unwrap();
        assert_eq!(aabb.min.to_array(), [-2.0, -2.0, -2.0]);
    }

    #[test]
    fn box_axis_aligned() {
        let r = Region::box_region(
            Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            Vec3::try_new(1.0, 2.0, 3.0).unwrap(),
            Mat3::identity(),
        )
        .unwrap();
        assert!(r.contains(Vec3::try_new(0.5, 1.5, 2.5).unwrap()));
        assert!(!r.contains(Vec3::try_new(1.5, 0.0, 0.0).unwrap()));
    }

    #[test]
    fn halfspace_ge_zero() {
        let r = Region::halfspace(Vec3::try_new(1.0, 0.0, 0.0).unwrap(), 0.0).unwrap();
        assert!(r.contains(Vec3::try_new(1.0, 0.0, 0.0).unwrap()));
        assert!(!r.contains(Vec3::try_new(-1.0, 0.0, 0.0).unwrap()));
    }

    #[test]
    fn csg_difference() {
        let a = Region::sphere(Vec3::try_new(0.0, 0.0, 0.0).unwrap(), 2.0).unwrap();
        let b = Region::sphere(Vec3::try_new(3.0, 0.0, 0.0).unwrap(), 2.0).unwrap();
        let d = Region::csg(CsgOp::Difference, vec![a, b]).unwrap();
        assert!(d.contains(Vec3::try_new(0.0, 0.0, 0.0).unwrap()));
        assert!(!d.contains(Vec3::try_new(1.5, 0.0, 0.0).unwrap()));
    }

    #[test]
    fn reject_negative_radius() {
        assert!(Region::sphere(Vec3::try_new(0.0, 0.0, 0.0).unwrap(), -1.0).is_err());
    }

    #[test]
    fn reject_non_rotation_region_frame() {
        let stretched =
            Mat3::try_from_row_major([2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]).unwrap();
        assert!(
            Region::box_region(
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                Vec3::try_new(1.0, 1.0, 1.0).unwrap(),
                stretched,
            )
            .is_err()
        );
    }
}
