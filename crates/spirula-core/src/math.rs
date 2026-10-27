//! Finite `f64` vectors, row-major matrices, and `wxyz` quaternions.
//!
//! The conventions are pinned in `docs/phase-1/geometry.md`.

use crate::{ContextKey, DomainError, ErrorCode};

const SINGULAR: f64 = 1e-12;
const ROTATION_TOL: f64 = 1e-9;

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

pub(crate) fn unimplemented_opt(field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::UnimplementedOption, message)
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn finite(value: f64) -> bool {
    value.is_finite()
}

fn near_identity(matrix: Mat3) -> bool {
    let identity = Mat3::identity().m;
    matrix
        .m
        .iter()
        .zip(identity)
        .all(|(actual, expected)| finite(*actual) && (actual - expected).abs() <= ROTATION_TOL)
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

    pub fn try_cross(self, other: Self) -> Result<Self, DomainError> {
        Self::try_new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn try_scale(self, factor: f64) -> Result<Self, DomainError> {
        Self::try_new(self.x * factor, self.y * factor, self.z * factor)
    }

    pub fn try_add(self, other: Self) -> Result<Self, DomainError> {
        Self::try_new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    pub fn try_sub(self, other: Self) -> Result<Self, DomainError> {
        Self::try_new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl std::ops::Add for Vec3 {
    type Output = Result<Self, DomainError>;

    fn add(self, other: Self) -> Self::Output {
        self.try_add(other)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Result<Self, DomainError>;

    fn sub(self, other: Self) -> Self::Output {
        self.try_sub(other)
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

    pub fn try_mul_vec(self, vector: Vec3) -> Result<Vec3, DomainError> {
        let m = &self.m;
        let x = vector.x();
        let y = vector.y();
        let z = vector.z();
        Vec3::try_new(
            m[0] * x + m[1] * y + m[2] * z,
            m[3] * x + m[4] * y + m[5] * z,
            m[6] * x + m[7] * y + m[8] * z,
        )
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

    pub fn try_mul(self, other: Self) -> Result<Self, DomainError> {
        let mut m = [0.0; 9];
        for row in 0..3 {
            for column in 0..3 {
                m[row * 3 + column] = self.m[row * 3] * other.m[column]
                    + self.m[row * 3 + 1] * other.m[3 + column]
                    + self.m[row * 3 + 2] * other.m[6 + column];
            }
        }
        Self::try_from_row_major(m)
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

    /// Proper rotation: both Gram matrices near `I`, determinant +1.
    ///
    /// Checking only `R^T R` is not transpose-stable under `1e-9` element
    /// tolerances, so `R R^T` is required as well.
    pub fn try_rotation(self) -> Result<Self, DomainError> {
        let gram_left = self
            .transpose()
            .try_mul(self)
            .map_err(|_| invalid("rotation", "matrix is not a proper rotation within 1e-9"))?;
        let gram_right = self
            .try_mul(self.transpose())
            .map_err(|_| invalid("rotation", "matrix is not a proper rotation within 1e-9"))?;
        if !near_identity(gram_left) || !near_identity(gram_right) {
            return Err(invalid(
                "rotation",
                "matrix is not a proper rotation within 1e-9",
            ));
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

impl std::ops::Mul for Mat3 {
    type Output = Result<Self, DomainError>;

    fn mul(self, other: Self) -> Self::Output {
        self.try_mul(other)
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

    /// Normalize a finite nonzero quaternion.
    ///
    /// Scales by the largest absolute component first so values such as
    /// `(1e200, 0, 0, 0)` and `(1e-13, 0, 0, 0)` stay finite and nonzero.
    pub fn try_from_wxyz(w: f64, x: f64, y: f64, z: f64) -> Result<Self, DomainError> {
        if !(finite(w) && finite(x) && finite(y) && finite(z)) {
            return Err(out_of_range("quaternion", "finite", "nonfinite"));
        }
        let max = w.abs().max(x.abs()).max(y.abs()).max(z.abs());
        if max == 0.0 {
            return Err(out_of_range("quaternion", "nonzero length", "0"));
        }
        let w = w / max;
        let x = x / max;
        let y = y / max;
        let z = z / max;
        let length = (w * w + x * x + y * y + z * z).sqrt();
        if !finite(length) || length == 0.0 {
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
        Mat3::try_from_row_major([
            1.0 - 2.0 * (yy + zz),
            2.0 * (xy - wz),
            2.0 * (xz + wy),
            2.0 * (xy + wz),
            1.0 - 2.0 * (xx + zz),
            2.0 * (yz - wx),
            2.0 * (xz - wy),
            2.0 * (yz + wx),
            1.0 - 2.0 * (xx + yy),
        ])
        .expect("unit quaternion yields a finite rotation")
    }

    pub fn try_rotate(self, vector: Vec3) -> Result<Vec3, DomainError> {
        self.to_rotation().try_mul_vec(vector)
    }
}

impl std::ops::Mul for Quat {
    type Output = Self;

    /// Hamilton product. `self * other` applies `other` first.
    ///
    /// The product is renormalized so repeated multiplication stays unit.
    fn mul(self, other: Self) -> Self {
        let w = self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z;
        let x = self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y;
        let y = self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x;
        let z = self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w;
        Self::try_from_wxyz(w, x, y, z).expect("unit quaternion product is nonzero")
    }
}
