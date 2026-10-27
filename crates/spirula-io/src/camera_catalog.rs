//! Camera fixture / catalog JSON (P3-F01). Preserves source params for rollback.
//!
//! Binding: `docs/phase-3/camera.md`. Does not invent goldens — loaders read
//! committed fixtures. Pure parse/serialize; math stays in `spirula-core`.

use serde_json::{Map, Value};
use spirula_core::{
    CameraModel, ContextKey, DISTORTION_PARAMS, DistortionTier, DomainError, ErrorCode, MAX_PARAMS,
    is_supported_source_model,
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
        let width = parse_positive_u32(obj.get("width"), "width")?;
        let height = parse_positive_u32(obj.get("height"), "height")?;
        let intr = obj
            .get("intrinsics")
            .and_then(|x| x.as_array())
            .ok_or_else(|| inp("intrinsics", "missing"))?;
        if intr.len() != 4 {
            return Err(inp("intrinsics", "expected 4 values"));
        }
        let fx = parse_positive_finite(intr[0].as_f64(), "fx")?;
        let fy = parse_positive_finite(intr[1].as_f64(), "fy")?;
        let cx = parse_finite(intr[2].as_f64(), "cx")?;
        let cy = parse_finite(intr[3].as_f64(), "cy")?;
        let mut coeffs = [0.0; DISTORTION_PARAMS];
        let c = obj
            .get("coeffs")
            .and_then(|x| x.as_array())
            .ok_or_else(|| inp("coeffs", "missing"))?;
        if c.len() != DISTORTION_PARAMS {
            return Err(inp("coeffs", "expected 8 values"));
        }
        for (i, item) in c.iter().enumerate() {
            coeffs[i] = parse_finite(item.as_f64(), "coeffs")?;
        }
        let source_model = parse_source_model(obj.get("source_model"))?;
        let source_params = parse_source_params(obj.get("source_params"))?;
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

fn parse_positive_u32(v: Option<&Value>, field: &str) -> Result<u32, DomainError> {
    let n = v
        .and_then(|x| x.as_u64())
        .ok_or_else(|| inp(field, "missing non-negative integer"))?;
    if n == 0 || n > u64::from(u32::MAX) {
        return Err(inp(field, format!("positive u32, got {n}")));
    }
    Ok(n as u32)
}

fn parse_finite(v: Option<f64>, field: &str) -> Result<f64, DomainError> {
    let x = v.ok_or_else(|| inp(field, "number"))?;
    if !x.is_finite() {
        return Err(inp(field, "finite number"));
    }
    Ok(x)
}

fn parse_positive_finite(v: Option<f64>, field: &str) -> Result<f64, DomainError> {
    let x = parse_finite(v, field)?;
    if x <= 0.0 {
        return Err(inp(field, "positive finite"));
    }
    Ok(x)
}

fn parse_source_model(v: Option<&Value>) -> Result<i32, DomainError> {
    let raw = match v {
        None => -1_i64,
        Some(Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| inp("source_model", "integer in i32 range"))?,
        Some(_) => return Err(inp("source_model", "integer")),
    };
    if raw < i64::from(i32::MIN) || raw > i64::from(i32::MAX) {
        return Err(inp("source_model", format!("i32 range, got {raw}")));
    }
    let id = raw as i32;
    if id != -1 && !is_supported_source_model(id) {
        return Err(inp(
            "source_model",
            format!("unsupported source model id {id}"),
        ));
    }
    Ok(id)
}

fn parse_source_params(v: Option<&Value>) -> Result<[f32; MAX_PARAMS], DomainError> {
    let arr = v
        .and_then(|x| x.as_array())
        .ok_or_else(|| inp("source_params", "missing array of length 16"))?;
    if arr.len() != MAX_PARAMS {
        return Err(inp(
            "source_params",
            format!("expected length {MAX_PARAMS}, got {}", arr.len()),
        ));
    }
    let mut out = [0.0f32; MAX_PARAMS];
    for (i, item) in arr.iter().enumerate() {
        let x = item
            .as_f64()
            .ok_or_else(|| inp("source_params", format!("slot {i} not a number")))?;
        if !x.is_finite() {
            return Err(inp("source_params", format!("slot {i} not finite")));
        }
        let f = x as f32;
        if !f.is_finite() {
            return Err(inp("source_params", format!("slot {i} not finite as f32")));
        }
        out[i] = f;
    }
    Ok(out)
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

    fn base_json() -> Value {
        serde_json::json!({
            "name": "cam",
            "model": "PINHOLE",
            "tier": "None",
            "width": 64,
            "height": 48,
            "intrinsics": [50.0, 50.0, 32.0, 24.0],
            "coeffs": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "source_model": -1,
            "source_params": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                              0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        })
    }

    #[test]
    fn rejects_width_overflow_and_zero() {
        let mut over = base_json();
        over["width"] = serde_json::json!(u64::from(u32::MAX) + 1);
        assert!(CameraCatalogEntry::from_json(&over).is_err());
        let mut zero = base_json();
        zero["width"] = serde_json::json!(0);
        assert!(CameraCatalogEntry::from_json(&zero).is_err());
        let mut hz = base_json();
        hz["height"] = serde_json::json!(0);
        assert!(CameraCatalogEntry::from_json(&hz).is_err());
    }

    #[test]
    fn rejects_nonpositive_or_nonfinite_focal() {
        let mut neg = base_json();
        neg["intrinsics"] = serde_json::json!([-1.0, 50.0, 32.0, 24.0]);
        assert!(CameraCatalogEntry::from_json(&neg).is_err());
        let mut nan = base_json();
        nan["intrinsics"] = serde_json::json!([50.0, f64::NAN, 32.0, 24.0]);
        assert!(CameraCatalogEntry::from_json(&nan).is_err());
        let mut zero_f = base_json();
        zero_f["intrinsics"] = serde_json::json!([0.0, 50.0, 32.0, 24.0]);
        assert!(CameraCatalogEntry::from_json(&zero_f).is_err());
    }

    #[test]
    fn rejects_source_model_wrap_and_unknown() {
        let mut wrap = base_json();
        wrap["source_model"] = serde_json::json!(4294967296_i64); // 2^32, outside i32
        assert!(CameraCatalogEntry::from_json(&wrap).is_err());
        let mut unk = base_json();
        unk["source_model"] = serde_json::json!(99);
        assert!(CameraCatalogEntry::from_json(&unk).is_err());
    }

    #[test]
    fn rejects_short_or_nonfinite_source_params() {
        let mut short = base_json();
        short["source_params"] = serde_json::json!([1.0, 2.0, 3.0]);
        assert!(CameraCatalogEntry::from_json(&short).is_err());
        let mut bad = base_json();
        let mut slots = vec![0.0; 16];
        slots[2] = f64::NAN;
        bad["source_params"] = serde_json::json!(slots);
        assert!(CameraCatalogEntry::from_json(&bad).is_err());
        let mut missing = base_json();
        missing.as_object_mut().unwrap().remove("source_params");
        assert!(CameraCatalogEntry::from_json(&missing).is_err());
    }
}
