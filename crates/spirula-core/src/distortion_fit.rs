//! Unrepresentable-lens fit (`dsfit`, C-DATA-13).
//!
//! Binding: `docs/phase-3/camera.md`. Full port of pinned
//! `src/data/DistortionFit.h/.cpp` (`9a24f099`): Chebyshev domain sample,
//! Levenberg-Marquardt + Lawson, coefficient ladder, auto model pick, and
//! empty-domain fisheye/`None` fallback.

use crate::DomainError;
use crate::camera_model::{
    CameraModel, DISTORTION_PARAMS, DistortionTier, RedistortDecision, is_compiled_pair,
    redistort_decision,
};
use crate::math::out_of_range;

/// Alias of [`crate::camera_model::EXACT_FIT_PX`] (`dsfit::kExactFitPx`).
pub use crate::camera_model::EXACT_FIT_PX as K_EXACT_FIT_PX;

const PI: f64 = std::f64::consts::PI;
const NC: usize = 8;
const NU: usize = 12;
const WIDEST_THETA: f64 = 175.0 * PI / 180.0;
const COEFF_LADDER: [u8; 6] = [0xff, 0x3f, 0x33, 0x03, 0x01, 0x00];

/// Fitted engine camera (`dsfit::FitTarget`).
///
/// Intrinsics / coeffs are stored as `f64` but assigned through `f32` casts to
/// match C++ `float` target storage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitTarget {
    pub model: CameraModel,
    pub distortion: DistortionTier,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub coeffs: [f64; DISTORTION_PARAMS],
}

impl FitTarget {
    fn blank(model: CameraModel) -> Self {
        Self {
            model,
            distortion: DistortionTier::ThinPrism,
            fx: 0.0,
            fy: 0.0,
            cx: 0.0,
            cy: 0.0,
            coeffs: [0.0; DISTORTION_PARAMS],
        }
    }

    fn from_model(model: &Model, camera: CameraModel) -> Self {
        let mut coeffs = [0.0; DISTORTION_PARAMS];
        for (slot, &c) in coeffs.iter_mut().zip(model.c.iter()) {
            *slot = f64::from(c as f32);
        }
        Self {
            model: camera,
            distortion: DistortionTier::ThinPrism,
            fx: f64::from(model.fx as f32),
            fy: f64::from(model.fy as f32),
            cx: f64::from(model.cx as f32),
            cy: f64::from(model.cy as f32),
            coeffs,
        }
    }
}

/// Fit quality (`dsfit::FitResult`).
#[derive(Clone, Debug, PartialEq)]
pub struct FitResult {
    pub target: FitTarget,
    pub rms_px: f64,
    pub max_px: f64,
    pub samples: u32,
    pub fov_deg: f64,
    pub invertible: bool,
    pub ok: bool,
}

impl FitResult {
    fn blank(model: CameraModel) -> Self {
        Self {
            target: FitTarget::blank(model),
            rms_px: 0.0,
            max_px: 0.0,
            samples: 0,
            fov_deg: 0.0,
            invertible: true,
            ok: false,
        }
    }

    pub fn redistort(&self) -> Result<RedistortDecision, DomainError> {
        redistort_decision(self.max_px)
    }
}

/// Options (`dsfit::FitOptions`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitOptions {
    pub grid: u32,
    pub refine_intrinsics: bool,
    pub max_rms_px: f64,
    pub lawson_iters: u32,
    pub coeff_mask: u8,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            grid: 64,
            refine_intrinsics: true,
            max_rms_px: 0.5,
            lawson_iters: 8,
            coeff_mask: 0xff,
        }
    }
}

/// Source forward map: view-space direction → pixel.
///
/// Call sites may pass `&impl Fn(...)` (or `&(dyn Fn(...) + '_)`) so the
/// closure may borrow local source params for the duration of the fit.
pub type SourceProject = dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError>;

#[derive(Clone, Copy)]
struct Model {
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    c: [f64; NC],
}

impl Default for Model {
    fn default() -> Self {
        Self {
            fx: 1.0,
            fy: 1.0,
            cx: 0.0,
            cy: 0.0,
            c: [0.0; NC],
        }
    }
}

#[derive(Clone)]
struct Sample {
    u: f64,
    v: f64,
    us: f64,
    vs: f64,
    w: f64,
    gx: [f64; NC],
    gy: [f64; NC],
}

impl Sample {
    fn new(u: f64, v: f64) -> Self {
        let mut gx = [0.0; NC];
        let mut gy = [0.0; NC];
        distort_basis(u, v, &mut gx, &mut gy);
        Self {
            u,
            v,
            us: 0.0,
            vs: 0.0,
            w: 1.0,
            gx,
            gy,
        }
    }
}

fn theta_cap(m: CameraModel) -> f64 {
    if m == CameraModel::Pinhole {
        85.0 * PI / 180.0
    } else {
        WIDEST_THETA
    }
}

fn rho_of_theta(m: CameraModel, th: f64) -> f64 {
    match m {
        CameraModel::Pinhole => th.tan(),
        CameraModel::Fisheye => th,
        CameraModel::Equisolid => 2.0 * (0.5 * th).sin(),
        CameraModel::Equirectangular => 0.0,
    }
}

fn unproject(m: CameraModel, u: f64, v: f64) -> Option<[f64; 3]> {
    match m {
        CameraModel::Pinhole => Some([u, v, 1.0]),
        CameraModel::Fisheye | CameraModel::Equisolid => {
            let r = (u * u + v * v).sqrt();
            let th = if m == CameraModel::Fisheye {
                r
            } else {
                if r > 2.0 {
                    return None;
                }
                2.0 * (0.5 * r).asin()
            };
            if th > PI {
                return None;
            }
            let s = if r > 1e-12 { th.sin() / r } else { 1.0 };
            Some([u * s, v * s, th.cos()])
        }
        CameraModel::Equirectangular => None,
    }
}

fn distort_basis(u: f64, v: f64, gx: &mut [f64; NC], gy: &mut [f64; NC]) {
    let r2 = u * u + v * v;
    let r4 = r2 * r2;
    let r6 = r4 * r2;
    let r8 = r4 * r4;
    gx[0] = u * r2;
    gx[1] = u * r4;
    gx[2] = u * r6;
    gx[3] = u * r8;
    gx[4] = 2.0 * u * v;
    gx[5] = r2 + 2.0 * u * u;
    gx[6] = r2;
    gx[7] = 0.0;
    gy[0] = v * r2;
    gy[1] = v * r4;
    gy[2] = v * r6;
    gy[3] = v * r8;
    gy[4] = r2 + 2.0 * v * v;
    gy[5] = 2.0 * u * v;
    gy[6] = 0.0;
    gy[7] = r2;
}

/// Fit invertibility: orientation-preserving Jacobian only (not CameraMath 0.25..4).
fn distortion_invertible(u: f64, v: f64, c: &[f64; NC]) -> bool {
    let r2 = u * u + v * v;
    let r4 = r2 * r2;
    let r6 = r4 * r2;
    let r = 1.0 + r2 * (c[0] + r2 * (c[1] + r2 * (c[2] + r2 * c[3])));
    let rp = c[0] + 2.0 * c[1] * r2 + 3.0 * c[2] * r4 + 4.0 * c[3] * r6;
    let p1 = c[4];
    let p2 = c[5];
    let sx = c[6];
    let sy = c[7];
    let j00 = r + 2.0 * u * u * rp + 2.0 * p1 * v + 6.0 * p2 * u + 2.0 * sx * u;
    let j01 = 2.0 * u * v * rp + 2.0 * p1 * u + 2.0 * p2 * v + 2.0 * sx * v;
    let j10 = 2.0 * u * v * rp + 2.0 * p2 * v + 2.0 * p1 * u + 2.0 * sy * u;
    let j11 = r + 2.0 * v * v * rp + 2.0 * p2 * u + 6.0 * p1 * v + 2.0 * sy * v;
    (j00 * j11 - j01 * j10).min(j00.min(j11)) > 0.0
}

fn solve_spd(n: usize, a: &mut [f64], b: &mut [f64]) -> bool {
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            if i == j {
                if s <= 0.0 {
                    return false;
                }
                a[i * n + i] = s.sqrt();
            } else {
                a[i * n + j] = s / a[j * n + j];
            }
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in (i + 1)..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    true
}

fn distorted(m: &Model, p: &Sample) -> (f64, f64) {
    let mut x = p.u;
    let mut y = p.v;
    for j in 0..NC {
        x += m.c[j] * p.gx[j];
        y += m.c[j] * p.gy[j];
    }
    (x, y)
}

fn cost(samples: &[Sample], m: &Model) -> f64 {
    let mut f = 0.0;
    for p in samples {
        let (dx, dy) = distorted(m, p);
        let ru = m.fx * dx + m.cx - p.us;
        let rv = m.fy * dy + m.cy - p.vs;
        f += p.w * (ru * ru + rv * rv);
    }
    f
}

fn optimize(samples: &[Sample], m: &mut Model, act: &[usize]) {
    let nu = act.len();
    if nu == 0 {
        return;
    }
    let mut lambda = 1e-6;
    let mut f0 = cost(samples, m);
    for _it in 0..200 {
        if lambda >= 1e14 {
            break;
        }
        let mut jtj = [0.0; NU * NU];
        let mut jtr = [0.0; NU];
        for p in samples {
            let (dx, dy) = distorted(m, p);
            let ru = m.fx * dx + m.cx - p.us;
            let rv = m.fy * dy + m.cy - p.vs;
            let mut ju = [0.0; NU];
            let mut jv = [0.0; NU];
            for j in 0..NC {
                ju[j] = m.fx * p.gx[j];
                jv[j] = m.fy * p.gy[j];
            }
            ju[8] = dx;
            ju[9] = 1.0;
            jv[10] = dy;
            jv[11] = 1.0;
            for i in 0..nu {
                let ai = act[i];
                jtr[i] += p.w * (ju[ai] * ru + jv[ai] * rv);
                for j in 0..=i {
                    let aj = act[j];
                    jtj[i * nu + j] += p.w * (ju[ai] * ju[aj] + jv[ai] * jv[aj]);
                }
            }
        }
        let mut stepped = false;
        for _attempt in 0..24 {
            if stepped || lambda >= 1e14 {
                break;
            }
            let mut a = [0.0; NU * NU];
            let mut b = [0.0; NU];
            a[..nu * nu].copy_from_slice(&jtj[..nu * nu]);
            for i in 0..nu {
                a[i * nu + i] += lambda * jtj[i * nu + i].max(1e-300);
                b[i] = -jtr[i];
            }
            if !solve_spd(nu, &mut a, &mut b) {
                lambda *= 10.0;
                continue;
            }
            let mut t = *m;
            for i in 0..nu {
                match act[i] {
                    8 => t.fx += b[i],
                    9 => t.cx += b[i],
                    10 => t.fy += b[i],
                    11 => t.cy += b[i],
                    idx => t.c[idx] += b[i],
                }
            }
            let f1 = cost(samples, &t);
            if f1 < f0 {
                let done = f0 - f1 <= 1e-14 * f0;
                *m = t;
                f0 = f1;
                lambda = (lambda * 0.1).max(1e-12);
                stepped = true;
                if done {
                    return;
                }
            } else {
                lambda *= 10.0;
            }
        }
        if !stepped {
            return;
        }
    }
}

fn solve_intrinsics(samples: &[Sample], m: &mut Model) {
    let mut sw = 0.0;
    let mut sxx = 0.0;
    let mut sx = 0.0;
    let mut sxu = 0.0;
    let mut su = 0.0;
    let mut syy = 0.0;
    let mut sy = 0.0;
    let mut syv = 0.0;
    let mut sv = 0.0;
    for p in samples {
        let (dx, dy) = distorted(m, p);
        sw += p.w;
        sxx += p.w * dx * dx;
        sx += p.w * dx;
        sxu += p.w * dx * p.us;
        su += p.w * p.us;
        syy += p.w * dy * dy;
        sy += p.w * dy;
        syv += p.w * dy * p.vs;
        sv += p.w * p.vs;
    }
    let dx_det = sxx * sw - sx * sx;
    if dx_det.abs() > 1e-30 * (sxx * sw + 1.0) {
        m.fx = (sxu * sw - sx * su) / dx_det;
        m.cx = (sxx * su - sx * sxu) / dx_det;
    }
    let dy_det = syy * sw - sy * sy;
    if dy_det.abs() > 1e-30 * (syy * sw + 1.0) {
        m.fy = (syv * sw - sy * sv) / dy_det;
        m.cy = (syy * sv - sy * syv) / dy_det;
    }
}

fn errors(samples: &[Sample], m: &Model, mut e: Option<&mut [f64]>) -> (f64, f64) {
    let mut sum2 = 0.0_f64;
    let mut worst = 0.0_f64;
    for (i, p) in samples.iter().enumerate() {
        let (dx, dy) = distorted(m, p);
        let ru = m.fx * dx + m.cx - p.us;
        let rv = m.fy * dy + m.cy - p.vs;
        let d = (ru * ru + rv * rv).sqrt();
        if let Some(buf) = e.as_mut() {
            buf[i] = d;
        }
        sum2 += ru * ru + rv * rv;
        worst = worst.max(d);
    }
    let rms = if samples.is_empty() {
        0.0
    } else {
        (sum2 / samples.len() as f64).sqrt()
    };
    (rms, worst)
}

struct Domain {
    theta: [f64; Self::AZ],
}

impl Domain {
    const AZ: usize = 128;

    fn new() -> Self {
        Self {
            theta: [0.0; Self::AZ],
        }
    }

    fn limit(&self, phi: f64) -> f64 {
        let mut t = phi * (Self::AZ as f64 / (2.0 * PI));
        t -= (t / Self::AZ as f64).floor() * Self::AZ as f64;
        let i = t as usize;
        let f = t - i as f64;
        self.theta[i % Self::AZ] * (1.0 - f) + self.theta[(i + 1) % Self::AZ] * f
    }
}

fn call_src(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    x: f64,
    y: f64,
    z: f64,
) -> Option<(f64, f64)> {
    src(x, y, z).ok()
}

fn scan_domain(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    w: f64,
    h: f64,
    cap: f64,
) -> Domain {
    let mut dom = Domain::new();
    let (mut cu, mut cv) = (0.5 * w, 0.5 * h);
    if let Some((u, v)) = call_src(src, 0.0, 0.0, 1.0) {
        cu = u;
        cv = v;
    }
    for a in 0..Domain::AZ {
        let phi = 2.0 * PI * a as f64 / Domain::AZ as f64;
        let cp = phi.cos();
        let sp = phi.sin();
        let radius = |th: f64| -> Option<f64> {
            let st = th.sin();
            let ct = th.cos();
            let (u, v) = call_src(src, st * cp, st * sp, ct)?;
            if !(u >= 0.0 && u <= w && v >= 0.0 && v <= h) {
                return None;
            }
            Some((u - cu).hypot(v - cv))
        };
        let back = cap * 1e-3;
        let r_ax = radius(back);
        let have_ax = r_ax.is_some();
        let min_growth = 0.25 * r_ax.unwrap_or(0.0);
        let inside = |th: f64| -> bool {
            let Some(r1) = radius(th) else {
                return false;
            };
            if th <= back {
                return true;
            }
            let Some(r0) = radius(th - back) else {
                return false;
            };
            if have_ax {
                r1 - r0 > min_growth
            } else {
                r1 > r0
            }
        };
        const STEPS: i32 = 48;
        let mut lo = -1.0;
        let mut hi = -1.0;
        for i in 0..=STEPS {
            let th = cap * i as f64 / STEPS as f64;
            if inside(th) {
                lo = th;
            } else if lo >= 0.0 {
                hi = th;
                break;
            }
        }
        if lo >= 0.0 && hi >= 0.0 {
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if inside(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
        }
        dom.theta[a] = lo.max(0.0);
    }
    dom
}

fn domain_max(d: &Domain) -> f64 {
    d.theta.iter().copied().fold(0.0, f64::max)
}

#[allow(clippy::too_many_arguments)]
fn build_samples(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    m: CameraModel,
    w: f64,
    h: f64,
    box4: [f64; 4],
    dom: &Domain,
    n: usize,
    chebyshev: bool,
) -> Vec<Sample> {
    let mut xs = vec![0.0; n];
    let mut ys = vec![0.0; n];
    for (i, (x, y)) in xs.iter_mut().zip(ys.iter_mut()).enumerate() {
        let t = if chebyshev {
            (PI * (2 * i + 1) as f64 / (2.0 * n as f64)).cos()
        } else if n == 1 {
            0.0
        } else {
            -1.0 + 2.0 * i as f64 / (n - 1) as f64
        };
        *x = 0.5 * (box4[0] + box4[1]) + 0.5 * (box4[1] - box4[0]) * t;
        *y = 0.5 * (box4[2] + box4[3]) + 0.5 * (box4[3] - box4[2]) * t;
    }
    let mut out = Vec::with_capacity(n * n);
    for &v in &ys {
        for &u in &xs {
            let Some(d) = unproject(m, u, v) else {
                continue;
            };
            let th = d[0].hypot(d[1]).atan2(d[2]);
            if th > dom.limit(v.atan2(u)) {
                continue;
            }
            let Some((us, vs)) = call_src(src, d[0], d[1], d[2]) else {
                continue;
            };
            if !(us >= 0.0 && us <= w && vs >= 0.0 && vs <= h) {
                continue;
            }
            let mut p = Sample::new(u, v);
            p.us = us;
            p.vs = vs;
            out.push(p);
        }
    }
    out
}

fn append_rim(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    m: CameraModel,
    w: f64,
    h: f64,
    dom: &Domain,
    out: &mut Vec<Sample>,
) {
    let kn = 4 * Domain::AZ;
    for a in 0..kn {
        let phi = 2.0 * PI * a as f64 / kn as f64;
        let rho = rho_of_theta(m, dom.limit(phi) * 0.999);
        let u = rho * phi.cos();
        let v = rho * phi.sin();
        let Some(d) = unproject(m, u, v) else {
            continue;
        };
        let Some((us, vs)) = call_src(src, d[0], d[1], d[2]) else {
            continue;
        };
        if !(us >= 0.0 && us <= w && vs >= 0.0 && vs <= h) {
            continue;
        }
        let mut p = Sample::new(u, v);
        p.us = us;
        p.vs = vs;
        out.push(p);
    }
}

fn active_unknowns(mask: u8, refine: bool) -> Vec<usize> {
    let mut act = Vec::new();
    for j in 0..NC {
        if mask & (1u8 << j) != 0 {
            act.push(j);
        }
    }
    if refine {
        act.extend_from_slice(&[8, 9, 10, 11]);
    }
    act
}

fn fit_one(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    w: f64,
    h: f64,
    model: CameraModel,
    opt: &FitOptions,
    mask: u8,
    dom: &Domain,
) -> FitResult {
    let mut res = FitResult::blank(model);

    let mut box4 = [0.0_f64; 4];
    for a in 0..Domain::AZ {
        let phi = 2.0 * PI * a as f64 / Domain::AZ as f64;
        let rho = rho_of_theta(model, dom.theta[a]);
        let x = rho * phi.cos();
        let y = rho * phi.sin();
        box4[0] = box4[0].min(x);
        box4[1] = box4[1].max(x);
        box4[2] = box4[2].min(y);
        box4[3] = box4[3].max(y);
    }
    if box4[1] <= box4[0] || box4[3] <= box4[2] {
        return res;
    }

    let n = (opt.grid as usize).clamp(8, 512);
    let mut fit = build_samples(src, model, w, h, box4, dom, n, true);
    if fit.len() < 4 * NC {
        return res;
    }

    let mut m = Model::default();
    solve_intrinsics(&fit, &mut m);
    if !(m.fx.is_finite() && m.fx > 0.0 && m.fy.is_finite() && m.fy > 0.0) {
        return res;
    }

    let act = active_unknowns(mask, opt.refine_intrinsics);
    let mut best = m;
    let mut best_max = 1e300;
    let lawson = opt.lawson_iters as i32;
    for round in 0..=lawson {
        optimize(&fit, &mut m, &act);
        let mut e = vec![0.0; fit.len()];
        let (_rms, mx) = errors(&fit, &m, Some(&mut e));
        if mx < best_max {
            best_max = mx;
            best = m;
        }
        if round == lawson {
            break;
        }
        let floor_e = 1e-3 * mx;
        let mut sum = 0.0;
        for i in 0..fit.len() {
            fit[i].w *= e[i] + floor_e;
            sum += fit[i].w;
        }
        if sum <= 0.0 {
            break;
        }
        let scale = fit.len() as f64 / sum;
        for p in &mut fit {
            p.w *= scale;
        }
    }

    let check_n = (2 * n + 1).min(1024);
    let mut check = build_samples(src, model, w, h, box4, dom, check_n, false);
    append_rim(src, model, w, h, dom, &mut check);
    if check.is_empty() {
        check = fit;
    }
    let (rms, mx) = errors(&check, &best, None);

    let invertible = check
        .iter()
        .all(|p| distortion_invertible(p.u, p.v, &best.c));

    res.target = FitTarget::from_model(&best, model);
    res.rms_px = rms;
    res.max_px = mx;
    res.samples = check.len() as u32;
    res.invertible = invertible;
    res.ok = invertible && rms <= opt.max_rms_px;
    res
}

fn fit_model(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    w: f64,
    h: f64,
    model: CameraModel,
    opt: &FitOptions,
    wide: &Domain,
) -> FitResult {
    let mut res = FitResult::blank(model);
    if !is_compiled_pair(model, DistortionTier::ThinPrism) {
        return res;
    }

    let mut dom = Domain::new();
    let cap = theta_cap(model);
    for a in 0..Domain::AZ {
        dom.theta[a] = wide.theta[a].min(cap);
    }

    let mut prev = !0u8;
    for rung in COEFF_LADDER {
        let mask = rung & opt.coeff_mask;
        if mask == prev {
            continue;
        }
        prev = mask;
        let r = fit_one(src, w, h, model, opt, mask, &dom);
        let invertible = r.invertible;
        if r.samples > 0 && (invertible || res.samples == 0) {
            res = r;
        }
        if invertible {
            break;
        }
    }
    res
}

/// Empty-domain / last-resort usable camera (`fit_camera_auto` fallback).
pub fn fallback_fisheye_none(width: u32, height: u32) -> Result<FitResult, DomainError> {
    if width == 0 || height == 0 {
        return Err(out_of_range(
            "image_size",
            "positive",
            format!("{width}x{height}"),
        ));
    }
    let w = f64::from(width);
    let h = f64::from(height);
    let fx = f64::from((0.5 * w.max(h)) as f32);
    let fy = fx;
    let cx = f64::from((0.5 * w) as f32);
    let cy = f64::from((0.5 * h) as f32);
    Ok(FitResult {
        target: FitTarget {
            model: CameraModel::Fisheye,
            distortion: DistortionTier::None,
            fx,
            fy,
            cx,
            cy,
            coeffs: [0.0; DISTORTION_PARAMS],
        },
        rms_px: 0.0,
        max_px: 0.0,
        samples: 0,
        fov_deg: 0.0,
        invertible: true,
        ok: false,
    })
}

/// Fit `target_model` + ThinPrism coefficients to `src` over `width`×`height`.
///
/// Equirectangular is rejected (`ok == false`); it carries no distortion tier
/// the fit can use.
pub fn fit_camera(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    width: u32,
    height: u32,
    target_model: CameraModel,
    opt: FitOptions,
) -> Result<FitResult, DomainError> {
    if width == 0 || height == 0 {
        return Err(out_of_range(
            "image_size",
            "positive",
            format!("{width}x{height}"),
        ));
    }
    let w = f64::from(width);
    let h = f64::from(height);
    let dom = scan_domain(src, w, h, theta_cap(target_model));
    let mut res = fit_model(src, w, h, target_model, &opt, &dom);
    res.fov_deg = 2.0 * domain_max(&dom) * 180.0 / PI;
    Ok(res)
}

/// Auto model selection with coefficient ladder and always-usable fallback.
pub fn fit_camera_auto(
    src: &(dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError> + '_),
    width: u32,
    height: u32,
    opt: FitOptions,
) -> Result<FitResult, DomainError> {
    if width == 0 || height == 0 {
        return Err(out_of_range(
            "image_size",
            "positive",
            format!("{width}x{height}"),
        ));
    }
    let w = f64::from(width);
    let h = f64::from(height);
    let dom = scan_domain(src, w, h, WIDEST_THETA);
    let half_fov = domain_max(&dom);
    let fov_deg = 2.0 * half_fov * 180.0 / PI;

    let mut cands = Vec::with_capacity(3);
    if half_fov < 60.0 * PI / 180.0 {
        cands.push(CameraModel::Pinhole);
    }
    cands.push(CameraModel::Fisheye);
    if half_fov > 70.0 * PI / 180.0 {
        cands.push(CameraModel::Equisolid);
    }

    const GOOD_PX: f64 = 0.1;
    let mut res = FitResult::blank(CameraModel::Pinhole);
    res.target.distortion = DistortionTier::ThinPrism;
    res.fov_deg = fov_deg;
    let mut have = false;
    for &cand in &cands {
        let r = fit_model(src, w, h, cand, &opt, &dom);
        if r.samples == 0 {
            continue;
        }
        let invertible = r.invertible;
        let max_px = r.max_px;
        let better = !have
            || (invertible && !res.invertible)
            || (invertible == res.invertible && max_px < 0.5 * res.max_px);
        if better {
            let fov = res.fov_deg;
            res = r;
            res.fov_deg = fov;
            have = true;
        }
        if invertible && max_px <= GOOD_PX {
            break;
        }
    }
    if have {
        return Ok(res);
    }

    let mut fallback = fallback_fisheye_none(width, height)?;
    fallback.fov_deg = fov_deg;
    Ok(fallback)
}

/// Fitted camera plus source metadata for redistort / rollback.
///
/// Exact fit (`max_px < EXACT_FIT_PX`) clears the **active** redistort source
/// to `-1` while preserving [`Self::original_source_model`] and the original
/// `source_params` bytes (ColmapParser / Nerfstudio binding).
#[derive(Clone, Debug, PartialEq)]
pub struct FittedWithSource {
    pub fitted: FitResult,
    /// Active redistort source id; `-1` when images stay as-is.
    pub source_model: i32,
    /// Source id before exact-fit clearing (always the caller-provided id).
    pub original_source_model: i32,
    pub source_params: [f32; 16],
    pub decision: RedistortDecision,
}

impl FittedWithSource {
    pub fn from_fit(
        fitted: FitResult,
        source_model: i32,
        source_params: [f32; 16],
    ) -> Result<Self, DomainError> {
        let decision = fitted.redistort()?;
        let active = match decision {
            RedistortDecision::KeepImagesAsIs => -1,
            RedistortDecision::KeepSourceForWarp => source_model,
        };
        Ok(Self {
            fitted,
            source_model: active,
            original_source_model: source_model,
            source_params,
            decision,
        })
    }

    pub fn redistort(&self) -> RedistortDecision {
        self.decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_camera::{self, COLMAP_FOV, MAX_PARAMS};

    #[test]
    fn exact_fit_constant() {
        assert_eq!(crate::camera_model::EXACT_FIT_PX, 0.1);
        assert_eq!(K_EXACT_FIT_PX, 0.1);
    }

    #[test]
    fn empty_domain_fallback_usable() {
        let r = fallback_fisheye_none(64, 48).unwrap();
        assert!(!r.ok);
        assert_eq!(r.samples, 0);
        assert_eq!(r.target.model, CameraModel::Fisheye);
        assert_eq!(r.target.distortion, DistortionTier::None);
        assert!(r.target.fx > 0.0);

        let empty = |_x, _y, _z| Err(out_of_range("source", "projectable", "empty"));
        let auto = fit_camera_auto(&empty, 1600, 1200, FitOptions::default()).unwrap();
        assert!(!auto.ok);
        assert_eq!(auto.samples, 0);
        assert_eq!(auto.target.model, CameraModel::Fisheye);
        assert_eq!(auto.target.distortion, DistortionTier::None);
        assert!((auto.target.fx - 800.0).abs() < 1e-6);
    }

    #[test]
    fn equirect_fit_rejected() {
        let src = |_x, _y, _z| Ok((32.0, 24.0));
        let r = fit_camera(
            &src,
            64,
            48,
            CameraModel::Equirectangular,
            FitOptions::default(),
        )
        .unwrap();
        assert!(!r.ok);
        assert_eq!(r.samples, 0);
        assert_eq!(r.target.model, CameraModel::Equirectangular);
    }

    #[test]
    fn fit_fov_fisheye_grid16_matches_order() {
        // Investigation golden: COLMAP FOV omega=0.9 @ 1600x1200, grid=16 →
        // Fisheye max_px ~ 1e-6 (well under 1e-3); source params unchanged.
        let params = {
            let mut p = [0.0f32; MAX_PARAMS];
            p[0] = 1000.0;
            p[1] = 1000.0;
            p[2] = 800.0;
            p[3] = 600.0;
            p[4] = 0.9;
            p
        };
        let params_before = params;

        // Borrow the same buffer so mutation would show up after the fit.
        let src = |x, y, z| source_camera::project(COLMAP_FOV, &params, x, y, z);
        let opt = FitOptions {
            grid: 16,
            ..FitOptions::default()
        };
        let r = fit_camera(&src, 1600, 1200, CameraModel::Fisheye, opt).unwrap();

        assert_eq!(r.target.model, CameraModel::Fisheye);
        assert_eq!(r.target.distortion, DistortionTier::ThinPrism);
        assert!(r.invertible);
        assert!(r.ok);
        assert!(
            r.max_px < 1e-3,
            "max_px {} should be ≪ 1e-3 (C++ ~3e-6)",
            r.max_px
        );
        assert!(
            r.rms_px < 1e-3,
            "rms_px {} should be ≪ 1e-3 (C++ ~1e-6)",
            r.rms_px
        );
        // ~1e-3 relative tolerance on intrinsics vs C++ float store.
        assert!((r.target.fx - 1073.456).abs() / 1073.456 < 1e-3);
        assert!((r.target.fy - 1073.456).abs() / 1073.456 < 1e-3);
        assert!((r.target.cx - 800.0).abs() < 1e-2);
        assert!((r.target.cy - 600.0).abs() < 1e-2);
        assert!((r.target.coeffs[0] - 0.022211).abs() < 5e-3);
        assert_eq!(params, params_before, "source params must not mutate");
    }

    #[test]
    fn keep_source_params_on_warp_decision() {
        let fitted = FitResult {
            target: FitTarget {
                model: CameraModel::Pinhole,
                distortion: DistortionTier::ThinPrism,
                fx: 100.0,
                fy: 100.0,
                cx: 32.0,
                cy: 24.0,
                coeffs: [0.0; DISTORTION_PARAMS],
            },
            rms_px: 0.2,
            max_px: 0.2,
            samples: 10,
            fov_deg: 90.0,
            invertible: true,
            ok: true,
        };
        let mut params = [0.0f32; 16];
        params[0] = 1.0;
        let wrapped = FittedWithSource::from_fit(fitted, 7, params).unwrap();
        assert_eq!(wrapped.source_model, 7);
        assert_eq!(wrapped.original_source_model, 7);
        assert_eq!(wrapped.decision, RedistortDecision::KeepSourceForWarp);
        assert_eq!(wrapped.source_params[0], 1.0);
        assert_eq!(wrapped.redistort(), RedistortDecision::KeepSourceForWarp);
    }

    #[test]
    fn clear_source_when_exact() {
        let fitted = FitResult {
            target: FitTarget {
                model: CameraModel::Pinhole,
                distortion: DistortionTier::OpenCv,
                fx: 100.0,
                fy: 100.0,
                cx: 32.0,
                cy: 24.0,
                coeffs: [0.0; DISTORTION_PARAMS],
            },
            rms_px: 0.01,
            max_px: 0.05,
            samples: 10,
            fov_deg: 60.0,
            invertible: true,
            ok: true,
        };
        let mut params = [0.0f32; 16];
        params[4] = 0.9;
        let wrapped = FittedWithSource::from_fit(fitted, 7, params).unwrap();
        assert_eq!(wrapped.source_model, -1);
        assert_eq!(wrapped.original_source_model, 7);
        assert_eq!(wrapped.source_params[4], 0.9);
        assert_eq!(wrapped.decision, RedistortDecision::KeepImagesAsIs);
    }

    #[test]
    fn fit_fov_auto_grid16_matches_investigation_golden() {
        // /tmp/.../golden.txt: FOV 1000/900/800/600/0.9 @1600x1200 grid16 →
        // FISHEYE ThinPrism, rms~1.53e-6, max~1.24e-5, n=1477.
        let params = {
            let mut p = [0.0f32; MAX_PARAMS];
            p[0] = 1000.0;
            p[1] = 900.0;
            p[2] = 800.0;
            p[3] = 600.0;
            p[4] = 0.9;
            p
        };
        let before = params;
        let src = |x, y, z| source_camera::project(COLMAP_FOV, &params, x, y, z);
        let r = fit_camera_auto(
            &src,
            1600,
            1200,
            FitOptions {
                grid: 16,
                ..FitOptions::default()
            },
        )
        .unwrap();
        assert_eq!(r.target.model, CameraModel::Fisheye);
        assert_eq!(r.target.distortion, DistortionTier::ThinPrism);
        assert!(r.ok && r.invertible);
        assert_eq!(r.samples, 1477);
        assert!(r.max_px < K_EXACT_FIT_PX);
        assert!(r.rms_px < 1e-4);
        assert!(r.max_px < 1e-3);
        assert_eq!(params, before);
        let wrapped = FittedWithSource::from_fit(r, COLMAP_FOV, params).unwrap();
        assert_eq!(wrapped.source_model, -1);
        assert_eq!(wrapped.original_source_model, COLMAP_FOV);
    }
}
