//! Camera fixture / catalog JSON (P3-F01). Preserves source params for rollback.
//!
//! Binding: `docs/phase-3/camera.md`. Does not invent goldens — loaders read
//! committed fixtures. Pure parse/serialize; math stays in `spirula-core`.

use serde_json::{Map, Value};
use spirula_core::{
    CameraModel, ContextKey, DISTORTION_PARAMS, DistortionTier, DomainError, ErrorCode, MAX_PARAMS,
};

fn inp(field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::InvalidArgument, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

/// One camera row as stored in fixtures (engine model + optional source lens).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraCatalogEntry {
    pub name: String,
    pub model: CameraModel,
    pub tier: DistortionTier,
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub coeffs: [f64; DISTORTION_PARAMS],
    /// `-1` means exact tier / keep images as-is (no source warp).
    pub source_model: i32,
    pub source_params: [f32; MAX_PARAMS],
}

impl CameraCatalogEntry {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("name".into(), Value::String(self.name.clone()));
        m.insert("model".into(), Value::String(self.model.as_str().into()));
        m.insert("tier".into(), Value::String(self.tier.as_str().into()));
        m.insert("width".into(), Value::from(self.width));
        m.insert("height".into(), Value::from(self.height));
        m.insert(
            "intrinsics".into(),
            Value::Array(
                [self.fx, self.fy, self.cx, self.cy]
                    .into_iter()
                    .map(Value::from)
                    .collect(),
            ),
        );
        m.insert(
            "coeffs".into(),
            Value::Array(self.coeffs.iter().copied().map(Value::from).collect()),
        );
        m.insert("source_model".into(), Value::from(self.source_model));
        m.insert(
            "source_params".into(),
            Value::Array(
                self.source_params
                    .iter()
                    .map(|p| Value::from(f64::from(*p)))
                    .collect(),
            ),
        );
        Value::Object(m)
    }

    pub fn from_json(v: &Value) -> Result<Self, DomainError> {
        let obj = v
            .as_object()
            .ok_or_else(|| inp("camera", "expected object"))?;
        let name = obj
            .get("name")
            .and_then(|x| x.as_str())
            .ok_or_else(|| inp("name", "missing"))?
            .to_string();
        let model = CameraModel::from_name(
            obj.get("model")
                .and_then(|x| x.as_str())
                .ok_or_else(|| inp("model", "missing"))?,
        )?;
        let tier = DistortionTier::from_name(
            obj.get("tier")
                .and_then(|x| x.as_str())
                .ok_or_else(|| inp("tier", "missing"))?,
        )?;
        let width = obj
            .get("width")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| inp("width", "missing"))? as u32;
        let height = obj
            .get("height")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| inp("height", "missing"))? as u32;
        let intr = obj
            .get("intrinsics")
            .and_then(|x| x.as_array())
            .ok_or_else(|| inp("intrinsics", "missing"))?;
        if intr.len() != 4 {
            return Err(inp("intrinsics", "expected 4 values"));
        }
        let fx = intr[0].as_f64().ok_or_else(|| inp("fx", "number"))?;
        let fy = intr[1].as_f64().ok_or_else(|| inp("fy", "number"))?;
        let cx = intr[2].as_f64().ok_or_else(|| inp("cx", "number"))?;
        let cy = intr[3].as_f64().ok_or_else(|| inp("cy", "number"))?;
        let mut coeffs = [0.0; DISTORTION_PARAMS];
        let c = obj
            .get("coeffs")
            .and_then(|x| x.as_array())
            .ok_or_else(|| inp("coeffs", "missing"))?;
        if c.len() != DISTORTION_PARAMS {
            return Err(inp("coeffs", "expected 8 values"));
        }
        for (i, item) in c.iter().enumerate() {
            coeffs[i] = item.as_f64().ok_or_else(|| inp("coeffs", "number"))?;
        }
        let source_model = obj
            .get("source_model")
            .and_then(|x| x.as_i64())
            .unwrap_or(-1) as i32;
        let mut source_params = [0.0f32; MAX_PARAMS];
        if let Some(sp) = obj.get("source_params").and_then(|x| x.as_array()) {
            for (i, item) in sp.iter().take(MAX_PARAMS).enumerate() {
                source_params[i] = item.as_f64().unwrap_or(0.0) as f32;
            }
        }
        Ok(Self {
            name,
            model,
            tier,
            width,
            height,
            fx,
            fy,
            cx,
            cy,
            coeffs,
            source_model,
            source_params,
        })
    }
}

/// Round-trip helper used by tests: JSON → entry → JSON keeps source params.
pub fn roundtrip_entry(entry: &CameraCatalogEntry) -> Result<CameraCatalogEntry, DomainError> {
    CameraCatalogEntry::from_json(&entry.to_json())
}

pub fn parse_catalog_bytes(bytes: &[u8]) -> Result<Vec<CameraCatalogEntry>, DomainError> {
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| inp("camera_catalog", format!("json parse: {e}")))?;
    let arr = v
        .get("cameras")
        .and_then(|x| x.as_array())
        .ok_or_else(|| inp("cameras", "missing array"))?;
    arr.iter().map(CameraCatalogEntry::from_json).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::is_compiled_pair;

    #[test]
    fn roundtrip_preserves_source_params() {
        let mut params = [0.0f32; MAX_PARAMS];
        params[0] = 100.0;
        params[4] = 0.7;
        let e = CameraCatalogEntry {
            name: "fov_fit".into(),
            model: CameraModel::Pinhole,
            tier: DistortionTier::ThinPrism,
            width: 64,
            height: 48,
            fx: 50.0,
            fy: 50.0,
            cx: 32.0,
            cy: 24.0,
            coeffs: [0.01, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            source_model: 7,
            source_params: params,
        };
        assert!(is_compiled_pair(e.model, e.tier));
        let back = roundtrip_entry(&e).unwrap();
        assert_eq!(back.source_model, 7);
        assert_eq!(back.source_params[0], 100.0);
        assert_eq!(back.source_params[4], 0.7);
        assert_eq!(back.model, CameraModel::Pinhole);
    }
}
