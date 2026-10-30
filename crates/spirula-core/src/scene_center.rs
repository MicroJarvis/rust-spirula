//! Scene-center modes (P3-F05). Pin: `SceneCenter.h` + `DatasetCommon.cpp`.

use crate::math::{invalid, out_of_range};
use crate::{DomainError, Vec3};

/// `--scene-center` / viewport centering menu names, in reference order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CenterMode {
    None = 0,
    PointMedian,
    CameraMedian,
    CameraFocus,
    PointMean,
    CameraMean,
}

pub const CENTER_MODE_NAMES: [&str; 6] = [
    "none",
    "point-median",
    "camera-median",
    "camera-focus",
    "point-mean",
    "camera-mean",
];

impl CenterMode {
    pub fn as_str(self) -> &'static str {
        CENTER_MODE_NAMES[self as usize]
    }

    /// Empty string is `None` (CLI spelling). Unknown names error.
    pub fn from_name(name: &str) -> Result<Self, DomainError> {
        if name.is_empty() {
            return Ok(Self::None);
        }
        for (i, n) in CENTER_MODE_NAMES.iter().enumerate() {
            if name == *n {
                return Ok(match i {
                    0 => Self::None,
                    1 => Self::PointMedian,
                    2 => Self::CameraMedian,
                    3 => Self::CameraFocus,
                    4 => Self::PointMean,
                    5 => Self::CameraMean,
                    _ => unreachable!(),
                });
            }
        }
        Err(invalid(
            "scene_center",
            format!("unknown scene center mode '{name}'"),
        ))
    }
}

/// Resolved centering choice and world-frame center.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedCenter {
    pub mode: CenterMode,
    pub center: Vec3,
}

fn sample_step(n: usize, max_samples: usize) -> usize {
    if max_samples == 0 || n <= max_samples {
        1
    } else {
        n.div_ceil(max_samples)
    }
}

fn median_of(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        0.5 * (values[mid - 1] + values[mid])
    } else {
        values[mid]
    }
}

fn mean_of(pos: &[[f64; 3]]) -> [f64; 3] {
    if pos.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    let mut s = [0.0; 3];
    for p in pos {
        for d in 0..3 {
            s[d] += p[d];
        }
    }
    let n = pos.len() as f64;
    [s[0] / n, s[1] / n, s[2] / n]
}

/// Weiszfeld geometric median from per-axis median init (~8 passes).
pub fn geometric_median(points: &[[f64; 3]], max_samples: usize) -> Result<Vec3, DomainError> {
    if points.is_empty() {
        return Vec3::try_new(0.0, 0.0, 0.0);
    }
    let step = sample_step(points.len(), max_samples);
    let mut y = [0.0; 3];
    for d in 0..3 {
        let col: Vec<f64> = points.iter().step_by(step).map(|p| p[d]).collect();
        y[d] = median_of(col);
    }
    for _ in 0..8 {
        let mut tx = 0.0;
        let mut ty = 0.0;
        let mut tz = 0.0;
        let mut dinvs = 0.0;
        let mut num_zeros = 0usize;
        let mut cnt = 0usize;
        for p in points.iter().step_by(step) {
            let dx = p[0] - y[0];
            let dy = p[1] - y[1];
            let dz = p[2] - y[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            cnt += 1;
            if dist == 0.0 {
                num_zeros += 1;
                continue;
            }
            let w = 1.0 / dist;
            dinvs += w;
            tx += w * p[0];
            ty += w * p[1];
            tz += w * p[2];
        }
        if num_zeros == cnt {
            break;
        }
        let mut wpt = [tx, ty, tz];
        if dinvs > 0.0 {
            for v in &mut wpt {
                *v /= dinvs;
            }
        }
        let y1 = if num_zeros == 0 {
            wpt
        } else {
            let mut rvec = [0.0; 3];
            let mut r = 0.0;
            for d in 0..3 {
                rvec[d] = (wpt[d] - y[d]) * dinvs;
                r += rvec[d] * rvec[d];
            }
            r = r.sqrt();
            let rinv = if r == 0.0 {
                0.0
            } else {
                (num_zeros as f64) / r
            };
            let a = (1.0 - rinv).max(0.0);
            let b = rinv.min(1.0);
            [
                a * wpt[0] + b * y[0],
                a * wpt[1] + b * y[1],
                a * wpt[2] + b * y[2],
            ]
        };
        let mut diff = 0.0;
        for d in 0..3 {
            diff += (y[d] - y1[d]) * (y[d] - y1[d]);
            y[d] = y1[d];
        }
        if diff == 0.0 {
            break;
        }
    }
    if y.iter().any(|v| !v.is_finite()) {
        return Err(out_of_range(
            "scene_center",
            "finite geometric median",
            "nonfinite",
        ));
    }
    Vec3::try_new(y[0], y[1], y[2])
}

/// Camera origins from OpenGL c2w 3×4 rows (`[N*12]`).
pub fn camera_positions(c2w: &[[f64; 12]]) -> Vec<[f64; 3]> {
    c2w.iter().map(|m| [m[3], m[7], m[11]]).collect()
}

/// Focus of attention: least-squares intersection of optical axes (−Z).
pub fn focus_of_attention(c2w: &[[f64; 12]], init: [f64; 3]) -> Result<Vec3, DomainError> {
    let mut focus = init;
    if c2w.is_empty() {
        return Vec3::try_new(focus[0], focus[1], focus[2]);
    }
    let org = camera_positions(c2w);
    let dir: Vec<[f64; 3]> = c2w.iter().map(|m| [-m[2], -m[6], -m[10]]).collect();
    let mut active: Vec<bool> = dir
        .iter()
        .zip(org.iter())
        .map(|(d, o)| {
            let s = d[0] * (focus[0] - o[0]) + d[1] * (focus[1] - o[1]) + d[2] * (focus[2] - o[2]);
            s > 0.0
        })
        .collect();
    let mut num_active = active.iter().filter(|&&a| a).count();
    while num_active > 1 {
        let mut a = [[0.0; 3]; 3];
        let mut b = [0.0; 3];
        for i in 0..c2w.len() {
            if !active[i] {
                continue;
            }
            let d = &dir[i];
            let o = &org[i];
            for r in 0..3 {
                for c in 0..3 {
                    let m = if r == c { 1.0 } else { 0.0 } - d[r] * d[c];
                    a[r][c] += m;
                    b[r] += m * o[c];
                }
            }
        }
        // Invert 3×3 via adjugate.
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if !det.is_finite() || det.abs() < 1e-18 {
            break;
        }
        let inv = [
            [
                (a[1][1] * a[2][2] - a[1][2] * a[2][1]) / det,
                (a[0][2] * a[2][1] - a[0][1] * a[2][2]) / det,
                (a[0][1] * a[1][2] - a[0][2] * a[1][1]) / det,
            ],
            [
                (a[1][2] * a[2][0] - a[1][0] * a[2][2]) / det,
                (a[0][0] * a[2][2] - a[0][2] * a[2][0]) / det,
                (a[0][2] * a[1][0] - a[0][0] * a[1][2]) / det,
            ],
            [
                (a[1][0] * a[2][1] - a[1][1] * a[2][0]) / det,
                (a[0][1] * a[2][0] - a[0][0] * a[2][1]) / det,
                (a[0][0] * a[1][1] - a[0][1] * a[1][0]) / det,
            ],
        ];
        for r in 0..3 {
            focus[r] = inv[r][0] * b[0] + inv[r][1] * b[1] + inv[r][2] * b[2];
        }
        let mut still = 0usize;
        for i in 0..c2w.len() {
            if !active[i] {
                continue;
            }
            let d = &dir[i];
            let o = &org[i];
            let s = d[0] * (focus[0] - o[0]) + d[1] * (focus[1] - o[1]) + d[2] * (focus[2] - o[2]);
            active[i] = s > 0.0;
            still += usize::from(active[i]);
        }
        if still == num_active {
            break;
        }
        num_active = still;
    }
    Vec3::try_new(focus[0], focus[1], focus[2])
}

/// Center for `mode` over cameras (`c2w` 3×4) and points. Cross-falls back when
/// the chosen cloud is empty; origin when both empty.
pub fn scene_center(
    mut mode: CenterMode,
    c2w: &[[f64; 12]],
    points: &[[f64; 3]],
) -> Result<Vec3, DomainError> {
    let n = c2w.len();
    let m = points.len();
    if m == 0 {
        if mode == CenterMode::PointMedian {
            mode = CenterMode::CameraMedian;
        }
        if mode == CenterMode::PointMean {
            mode = CenterMode::CameraMean;
        }
    }
    if n == 0 {
        if mode == CenterMode::CameraMedian {
            mode = CenterMode::PointMedian;
        }
        if mode == CenterMode::CameraMean || mode == CenterMode::CameraFocus {
            mode = CenterMode::PointMean;
        }
        if m == 0 {
            mode = CenterMode::None;
        }
    }
    match mode {
        CenterMode::None => Vec3::try_new(0.0, 0.0, 0.0),
        CenterMode::PointMedian => geometric_median(points, 0),
        CenterMode::PointMean => {
            let mean = mean_of(points);
            Vec3::try_new(mean[0], mean[1], mean[2])
        }
        CenterMode::CameraMedian => {
            let pos = camera_positions(c2w);
            geometric_median(&pos, 0)
        }
        CenterMode::CameraMean => {
            let pos = camera_positions(c2w);
            let mean = mean_of(&pos);
            Vec3::try_new(mean[0], mean[1], mean[2])
        }
        CenterMode::CameraFocus => {
            let pos = camera_positions(c2w);
            let mean = mean_of(&pos);
            focus_of_attention(c2w, mean)
        }
    }
}

fn far_from_origin(pos: &[[f64; 3]], c: [f64; 3], threshold: f32, step: usize) -> bool {
    let mut dist = Vec::new();
    for p in pos.iter().step_by(step) {
        let dx = p[0] - c[0];
        let dy = p[1] - c[1];
        let dz = p[2] - c[2];
        dist.push((dx * dx + dy * dy + dz * dz).sqrt());
    }
    let radius = median_of(dist);
    (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() > f64::from(threshold) * radius
}

/// Named mode, or `auto`: point/camera median when far from origin, else none.
pub fn resolve_scene_center(
    name: &str,
    auto_threshold: f32,
    c2w: &[[f64; 12]],
    points: &[[f64; 3]],
) -> Result<ResolvedCenter, DomainError> {
    if !auto_threshold.is_finite() || auto_threshold <= 0.0 {
        return Err(out_of_range(
            "scene_center_threshold",
            "positive finite",
            format!("{auto_threshold}"),
        ));
    }
    if name != "auto" {
        let mode = CenterMode::from_name(name)?;
        let center = scene_center(mode, c2w, points)?;
        return Ok(ResolvedCenter { mode, center });
    }
    let n = c2w.len();
    let m = points.len();
    if n > 0 {
        let cams = camera_positions(c2w);
        let c = geometric_median(&cams, 0)?;
        let ca = c.to_array();
        if !far_from_origin(&cams, ca, auto_threshold, 1) {
            return Ok(ResolvedCenter {
                mode: CenterMode::None,
                center: Vec3::try_new(0.0, 0.0, 0.0)?,
            });
        }
        if m == 0 {
            return Ok(ResolvedCenter {
                mode: CenterMode::CameraMedian,
                center: c,
            });
        }
    }
    if m == 0 {
        return Ok(ResolvedCenter {
            mode: CenterMode::None,
            center: Vec3::try_new(0.0, 0.0, 0.0)?,
        });
    }
    const K_SAMPLES: usize = 1 << 18;
    let c = geometric_median(points, K_SAMPLES)?;
    let ca = c.to_array();
    let step = sample_step(m, K_SAMPLES);
    if !far_from_origin(points, ca, auto_threshold, step) {
        return Ok(ResolvedCenter {
            mode: CenterMode::None,
            center: Vec3::try_new(0.0, 0.0, 0.0)?,
        });
    }
    let center = if m <= K_SAMPLES {
        c
    } else {
        geometric_median(points, 0)?
    };
    Ok(ResolvedCenter {
        mode: CenterMode::PointMedian,
        center,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_mode_names_roundtrip() {
        assert_eq!(CenterMode::from_name("").unwrap(), CenterMode::None);
        assert_eq!(
            CenterMode::from_name("point-median").unwrap(),
            CenterMode::PointMedian
        );
        assert!(CenterMode::from_name("sideways").is_err());
    }

    #[test]
    fn none_is_origin() {
        let c = scene_center(CenterMode::None, &[], &[]).unwrap();
        assert_eq!(c.to_array(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn point_mean_and_median() {
        let pts = [[1.0, 0.0, 0.0], [3.0, 0.0, 0.0], [100.0, 0.0, 0.0]];
        let mean = scene_center(CenterMode::PointMean, &[], &pts).unwrap();
        assert!((mean.x() - 104.0 / 3.0).abs() < 1e-12);
        let med = scene_center(CenterMode::PointMedian, &[], &pts).unwrap();
        assert!((med.x() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn point_mode_falls_back_to_cameras() {
        let c2w = [[1.0, 0.0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]];
        let c = scene_center(CenterMode::PointMean, &c2w, &[]).unwrap();
        assert_eq!(c.to_array(), [2.0, 0.0, 0.0]);
    }

    #[test]
    fn auto_none_when_near_origin() {
        let pts = [[0.1, 0.0, 0.0], [-0.1, 0.0, 0.0]];
        let r = resolve_scene_center("auto", 20.0, &[], &pts).unwrap();
        assert_eq!(r.mode, CenterMode::None);
    }
}
