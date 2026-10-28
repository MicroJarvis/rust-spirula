//! COLMAP world→camera quaternion → OpenGL camera-to-world (`colmap_to_c2w`).

use spirula_core::{ContextKey, DomainError, ErrorCode};

/// `qvec2rotmat` — world→camera rotation from `(w, x, y, z)`.
pub fn qvec2rotmat(q: [f64; 4]) -> [[f64; 3]; 3] {
    let [w, x, y, z] = q;
    [
        [
            1.0 - 2.0 * y * y - 2.0 * z * z,
            2.0 * x * y - 2.0 * w * z,
            2.0 * x * z + 2.0 * w * y,
        ],
        [
            2.0 * x * y + 2.0 * w * z,
            1.0 - 2.0 * x * x - 2.0 * z * z,
            2.0 * y * z - 2.0 * w * x,
        ],
        [
            2.0 * x * z - 2.0 * w * y,
            2.0 * y * z + 2.0 * w * x,
            1.0 - 2.0 * x * x - 2.0 * y * y,
        ],
    ]
}

/// OpenCV world→camera → OpenGL c2w 3×4 row-major (`ColmapParser.cpp:517-530`).
///
/// `c2w[:3,:3] = R^T` with columns 1 and 2 negated; `c2w[:3,3] = -(R^T) @ t`.
pub fn colmap_to_c2w(qvec: [f64; 4], tvec: [f64; 3]) -> Result<[f64; 12], DomainError> {
    if !qvec.iter().all(|v| v.is_finite()) || !tvec.iter().all(|v| v.is_finite()) {
        return Err(
            DomainError::try_new(ErrorCode::ValueOutOfRange, "nonfinite COLMAP pose")
                .expect("msg")
                .with_context(ContextKey::Field, "pose")
                .expect("field"),
        );
    }
    let r = qvec2rotmat(qvec);
    let flip = [1.0, -1.0, -1.0];
    let mut out = [0.0; 12];
    for row in 0..3 {
        for col in 0..3 {
            out[row * 4 + col] = r[col][row] * flip[col];
        }
        let mut t = 0.0;
        for col in 0..3 {
            t += r[col][row] * tvec[col];
        }
        out[row * 4 + 3] = -t;
    }
    if !out.iter().all(|v| v.is_finite()) {
        return Err(
            DomainError::try_new(ErrorCode::ValueOutOfRange, "nonfinite OpenGL c2w")
                .expect("msg")
                .with_context(ContextKey::Field, "c2w")
                .expect("field"),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_qvec_zero_t_is_flip_only() {
        let c2w = colmap_to_c2w([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0]).unwrap();
        // R = I → R^T = I; flip y,z → diag(1,-1,-1); t = 0
        assert!((c2w[0] - 1.0).abs() < 1e-15);
        assert!((c2w[5] + 1.0).abs() < 1e-15);
        assert!((c2w[10] + 1.0).abs() < 1e-15);
        assert_eq!(c2w[3], 0.0);
        assert_eq!(c2w[7], 0.0);
        assert_eq!(c2w[11], 0.0);
    }

    #[test]
    fn nonidentity_pose_matches_pin_formula() {
        let q = [0.9238795325112867, 0.0, 0.3826834323650898, 0.0]; // 45° about Y
        let t = [1.0, 2.0, 3.0];
        let c2w = colmap_to_c2w(q, t).unwrap();
        let r = qvec2rotmat(q);
        let flip = [1.0, -1.0, -1.0];
        for row in 0..3 {
            for col in 0..3 {
                let expect = r[col][row] * flip[col];
                assert!((c2w[row * 4 + col] - expect).abs() < 1e-12);
            }
            let mut tt = 0.0;
            for col in 0..3 {
                tt += r[col][row] * t[col];
            }
            assert!((c2w[row * 4 + 3] + tt).abs() < 1e-12);
        }
    }

    #[test]
    fn rejects_nonfinite() {
        let e = colmap_to_c2w([1.0, f64::NAN, 0.0, 0.0], [0.0; 3]).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
    }
}
