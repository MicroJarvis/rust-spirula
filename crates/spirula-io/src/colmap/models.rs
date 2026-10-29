//! Eighteen COLMAP source camera models (pin `ColmapParser.cpp:72-100`).

use spirula_core::{ContextKey, DomainError, ErrorCode};

/// One COLMAP model id → name + parameter count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColmapModelInfo {
    pub id: i32,
    pub name: &'static str,
    pub num_params: usize,
}

/// Fixed table matching `colmap_model_table()` (ids 0..=17).
pub const COLMAP_MODELS: &[ColmapModelInfo] = &[
    ColmapModelInfo {
        id: 0,
        name: "SIMPLE_PINHOLE",
        num_params: 3,
    },
    ColmapModelInfo {
        id: 1,
        name: "PINHOLE",
        num_params: 4,
    },
    ColmapModelInfo {
        id: 2,
        name: "SIMPLE_RADIAL",
        num_params: 4,
    },
    ColmapModelInfo {
        id: 3,
        name: "RADIAL",
        num_params: 5,
    },
    ColmapModelInfo {
        id: 4,
        name: "OPENCV",
        num_params: 8,
    },
    ColmapModelInfo {
        id: 5,
        name: "OPENCV_FISHEYE",
        num_params: 8,
    },
    ColmapModelInfo {
        id: 6,
        name: "FULL_OPENCV",
        num_params: 12,
    },
    ColmapModelInfo {
        id: 7,
        name: "FOV",
        num_params: 5,
    },
    ColmapModelInfo {
        id: 8,
        name: "SIMPLE_RADIAL_FISHEYE",
        num_params: 4,
    },
    ColmapModelInfo {
        id: 9,
        name: "RADIAL_FISHEYE",
        num_params: 5,
    },
    ColmapModelInfo {
        id: 10,
        name: "THIN_PRISM_FISHEYE",
        num_params: 12,
    },
    ColmapModelInfo {
        id: 11,
        name: "RAD_TAN_THIN_PRISM_FISHEYE",
        num_params: 16,
    },
    ColmapModelInfo {
        id: 12,
        name: "SIMPLE_DIVISION",
        num_params: 4,
    },
    ColmapModelInfo {
        id: 13,
        name: "DIVISION",
        num_params: 5,
    },
    ColmapModelInfo {
        id: 14,
        name: "SIMPLE_FISHEYE",
        num_params: 3,
    },
    ColmapModelInfo {
        id: 15,
        name: "FISHEYE",
        num_params: 4,
    },
    ColmapModelInfo {
        id: 16,
        name: "EUCM",
        num_params: 6,
    },
    ColmapModelInfo {
        id: 17,
        name: "EQUIRECTANGULAR",
        num_params: 2,
    },
];

pub fn model_by_id(id: i32) -> Option<&'static ColmapModelInfo> {
    COLMAP_MODELS.iter().find(|m| m.id == id)
}

pub fn model_id_by_name(name: &str) -> Option<i32> {
    COLMAP_MODELS.iter().find(|m| m.name == name).map(|m| m.id)
}

pub fn require_model_by_id(id: i32) -> Result<&'static ColmapModelInfo, DomainError> {
    model_by_id(id).ok_or_else(|| {
        DomainError::try_new(
            ErrorCode::UnsupportedFormat,
            format!("unknown camera model id {id}"),
        )
        .expect("msg")
        .with_context(ContextKey::Field, "model_id")
        .expect("field")
        .with_context(ContextKey::Actual, id.to_string())
        .expect("actual")
    })
}

pub fn require_model_by_name(name: &str) -> Result<&'static ColmapModelInfo, DomainError> {
    COLMAP_MODELS
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::UnsupportedFormat,
                format!("unknown camera model {name}"),
            )
            .expect("msg")
            .with_context(ContextKey::Field, "model")
            .expect("field")
            .with_context(ContextKey::Actual, name.to_owned())
            .expect("actual")
        })
}

pub fn require_model_id_by_name(name: &str) -> Result<i32, DomainError> {
    Ok(require_model_by_name(name)?.id)
}

/// Exact parameter arity from the 18-model table (text/binary consistency).
pub fn require_exact_param_count(
    info: &ColmapModelInfo,
    params: &[f64],
) -> Result<(), DomainError> {
    if params.len() != info.num_params {
        return Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!(
                "camera model {} needs exactly {} parameters, got {}",
                info.name,
                info.num_params,
                params.len()
            ),
        )
        .expect("msg")
        .with_context(ContextKey::Field, "params")
        .expect("field")
        .with_context(ContextKey::Expected, info.num_params.to_string())
        .expect("expected")
        .with_context(ContextKey::Actual, params.len().to_string())
        .expect("actual"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighteen_models_cover_0_to_17() {
        assert_eq!(COLMAP_MODELS.len(), 18);
        for (i, m) in COLMAP_MODELS.iter().enumerate() {
            assert_eq!(m.id, i as i32);
            assert_eq!(model_id_by_name(m.name), Some(m.id));
            assert_eq!(model_by_id(m.id).map(|x| x.name), Some(m.name));
        }
    }
}
