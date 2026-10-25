//! Finite `f64` vectors, row-major matrices, and `wxyz` quaternions.
//!
//! The conventions are pinned in `docs/phase-1/geometry.md`.

use crate::{ContextKey, DomainError, ErrorCode};

const SINGULAR: f64 = 1e-12;
const ROTATION_TOL: f64 = 1e-9;
const QUAT_MIN_LENGTH: f64 = 1e-12;

pub(crate) fn invalid(field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::InvalidArgument, message)
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

pub(crate) fn out_of_range(field: &str, expected: &str, actual: impl Into<String>) -> DomainError {
    DomainError::try_new(
        ErrorCode::ValueOutOfRange,
        format!("{field} is out of range"),
    )
    .expect("message")
    .with_context(ContextKey::Field, field)
    .expect("field")
    .with_context(ContextKey::Expected, expected)
    .expect("expected")
    .with_context(ContextKey::Actual, actual.into())
    .expect("actual")
}

fn finite(value: f64) -> bool {
    value.is_finite()
}

/// Finite 3-vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}

impl Vec3 {
    pub fn try_new(x: f64, y: f64, z: f64) -> Result<Self, DomainError> {
        if finite(x) && finite(y) && finite(z) {
            Ok(Self { x, y, z })
        } else {
            Err(out_of_range("vector", "finite", "nonfinite"))
        }
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }

    pub const fn z(self) -> f64 {
        self.z
    }

    pub const fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn scale(self, factor: f64) -> Self {
        Self {
            x: self.x * factor,
            y: self.y * factor,
            z: self.z * factor,
        }
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

/// Finite 2-vector, used for pixels and normalized image coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 {
    x: f64,
    y: f64,
}

impl Vec2 {
    pub fn try_new(x: f64, y: f64) -> Result<Self, DomainError> {
        if finite(x) && finite(y) {
            Ok(Self { x, y })
        } else {
            Err(out_of_range("vector", "finite", "nonfinite"))
        }
    }

    pub(crate) fn new_finite(x: f64, y: f64) -> Result<Self, DomainError> {
        Self::try_new(x, y)
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }
}

/// Row-major 3×3 matrix. Element `(row, column)` is index `row * 3 + column`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3 {
    m: [f64; 9],
}

impl Mat3 {
    pub fn identity() -> Self {
        Self {
            m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    pub fn try_from_row_major(m: [f64; 9]) -> Result<Self, DomainError> {
        if m.iter().all(|value| finite(*value)) {
            Ok(Self { m })
        } else {
            Err(out_of_range("matrix", "finite", "nonfinite"))
        }
    }

    /// Column-major storage is the transpose of the row-major matrix.
    pub fn try_from_column_major(column_major: [f64; 9]) -> Result<Self, DomainError> {
        let mut m = [0.0; 9];
        for row in 0..3 {
            for column in 0..3 {
                m[row * 3 + column] = column_major[column * 3 + row];
            }
        }
        Self::try_from_row_major(m)
    }

    pub const fn row_major(self) -> [f64; 9] {
        self.m
    }

    pub fn column_major(self) -> [f64; 9] {
        let mut out = [0.0; 9];
        for row in 0..3 {
            for column in 0..3 {
                out[column * 3 + row] = self.m[row * 3 + column];
            }
        }
        out
    }

    pub fn mul_vec(self, vector: Vec3) -> Vec3 {
        let m = &self.m;
        let x = vector.x();
        let y = vector.y();
        let z = vector.z();
        Vec3 {
            x: m[0] * x + m[1] * y + m[2] * z,
            y: m[3] * x + m[4] * y + m[5] * z,
            z: m[6] * x + m[7] * y + m[8] * z,
        }
    }

    pub fn transpose(self) -> Self {
        Self {
            m: self.column_major(),
        }
    }

    pub fn determinant(self) -> f64 {
        let m = &self.m;
        m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6])
    }

    pub fn try_inverse(self) -> Result<Self, DomainError> {
        let det = self.determinant();
        if !finite(det) || det.abs() < SINGULAR {
            return Err(out_of_range("determinant", "nonzero", format!("{det}")));
        }
        let m = &self.m;
        let inv = [
            (m[4] * m[8] - m[5] * m[7]) / det,
            (m[2] * m[7] - m[1] * m[8]) / det,
            (m[1] * m[5] - m[2] * m[4]) / det,
            (m[5] * m[6] - m[3] * m[8]) / det,
            (m[0] * m[8] - m[2] * m[6]) / det,
            (m[2] * m[3] - m[0] * m[5]) / det,
            (m[3] * m[7] - m[4] * m[6]) / det,
            (m[1] * m[6] - m[0] * m[7]) / det,
            (m[0] * m[4] - m[1] * m[3]) / det,
        ];
        Self::try_from_row_major(inv)
    }
}

impl std::ops::Mul for Mat3 {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        let mut m = [0.0; 9];
        for row in 0..3 {
            for column in 0..3 {
                m[row * 3 + column] = self.m[row * 3] * other.m[column]
                    + self.m[row * 3 + 1] * other.m[3 + column]
                    + self.m[row * 3 + 2] * other.m[6 + column];
            }
        }
        Self { m }
    }
}

impl Mat3 {
    /// Proper rotation: orthonormal columns and determinant +1.
    pub fn try_rotation(self) -> Result<Self, DomainError> {
        let gram = self.transpose() * self;
        let identity = Self::identity().m;
        for (actual, expected) in gram.m.iter().zip(identity) {
            if !finite(*actual) || (actual - expected).abs() > ROTATION_TOL {
                return Err(invalid(
                    "rotation",
                    "matrix is not a proper rotation within 1e-9",
                ));
            }
        }
        let det = self.determinant();
        if !finite(det) || (det - 1.0).abs() > ROTATION_TOL {
            return Err(invalid(
                "rotation",
                "matrix is not a proper rotation within 1e-9",
            ));
        }
        Ok(self)
    }
}

/// Unit quaternion in `(w, x, y, z)` order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    w: f64,
    x: f64,
    y: f64,
    z: f64,
}

impl Quat {
    pub fn identity() -> Self {
        Self {
            w: 1.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    /// Normalize a finite quaternion. A near-zero length is `SP-INP-003`.
    pub fn try_from_wxyz(w: f64, x: f64, y: f64, z: f64) -> Result<Self, DomainError> {
        if !(finite(w) && finite(x) && finite(y) && finite(z)) {
            return Err(out_of_range("quaternion", "finite", "nonfinite"));
        }
        let length = (w * w + x * x + y * y + z * z).sqrt();
        if !finite(length) || length < QUAT_MIN_LENGTH {
            return Err(out_of_range(
                "quaternion",
                "nonzero length",
                format!("{length}"),
            ));
        }
        Ok(Self {
            w: w / length,
            x: x / length,
            y: y / length,
            z: z / length,
        })
    }

    pub const fn wxyz(self) -> [f64; 4] {
        [self.w, self.x, self.y, self.z]
    }

    pub const fn xyzw(self) -> [f64; 4] {
        [self.x, self.y, self.z, self.w]
    }

    pub fn conjugate(self) -> Self {
        Self {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    pub fn to_rotation(self) -> Mat3 {
        let (w, x, y, z) = (self.w, self.x, self.y, self.z);
        let xx = x * x;
        let yy = y * y;
        let zz = z * z;
        let xy = x * y;
        let xz = x * z;
        let yz = y * z;
        let wx = w * x;
        let wy = w * y;
        let wz = w * z;
        Mat3 {
            m: [
                1.0 - 2.0 * (yy + zz),
                2.0 * (xy - wz),
                2.0 * (xz + wy),
                2.0 * (xy + wz),
                1.0 - 2.0 * (xx + zz),
                2.0 * (yz - wx),
                2.0 * (xz - wy),
                2.0 * (yz + wx),
                1.0 - 2.0 * (xx + yy),
            ],
        }
    }

    pub fn rotate(self, vector: Vec3) -> Vec3 {
        self.to_rotation().mul_vec(vector)
    }
}

impl std::ops::Mul for Quat {
    type Output = Self;

    /// Hamilton product. `self * other` applies `other` first.
    fn mul(self, other: Self) -> Self {
        Self {
            w: self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
            x: self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            y: self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            z: self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
        }
    }
}
