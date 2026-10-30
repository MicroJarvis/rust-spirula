//! Scene-transform JSON (`spirula-scene-transform` v1). Pin: `SceneTransform.cpp`.

use std::path::Path;

use serde_json::{Map, Number, Value};
use spirula_core::{
    CancelToken, ContextKey, DomainError, ErrorCode, Mat3, Quat, SceneTransform, Vec3,
};

use crate::atomic::{PublishOptions, publish_bytes_validated};

pub const SCENE_TRANSFORM_FORMAT: &str = "spirula-scene-transform";
pub const SCENE_TRANSFORM_VERSION: u64 = 1;

const README: &[&str] = &[
    "p_train = train_from_world.scale * train_from_world.rotation.matrix_3x3 @ p_world + train_from_world.translation, for a column vector p.",
    "train_from_world is the ACTIVE transform taking a point of the dataset's own frame (world) into the frame the splats were trained in; read passively, it gives a fixed point's train coordinates from its world coordinates.",
    "world_from_train is its inverse.",
    "matrix_4x4 acts on column vectors [x, y, z, 1].",
    "rotation.matrix_3x3 acts on column vectors; quaternion_wxyz is (w, x, y, z) and quaternion_xyzw is (x, y, z, w).",
    "translation is applied after rotation and scale; translation_before_rotation is the equivalent shift applied first.",
    "georeference places the world frame on Earth: x east, y north, z up, metres from origin_lat/origin_lon/origin_alt.",
];

const EULER_ORDERS: [&str; 6] = ["xyz", "xzy", "yxz", "yzx", "zxy", "zyx"];
const DEG: f64 = 180.0 / std::f64::consts::PI;

/// Optional ENU georeference block.
#[derive(Clone, Debug, PartialEq)]
pub struct GeoReference {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub origin_alt: Option<f64>,
    pub alt_source: Option<String>,
}

/// Full document: active similarity + centering + optional georeference.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneTransformDoc {
    pub train_from_world: SceneTransform,
    pub center_mode: String,
    pub center_world: [f64; 3],
    pub georeference: Option<GeoReference>,
}

fn err(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn json_num(v: f64) -> Value {
    let v = if v == 0.0 { 0.0 } else { v };
    Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn arr3(v: [f64; 3]) -> Value {
    Value::Array(vec![json_num(v[0]), json_num(v[1]), json_num(v[2])])
}

fn arr_n(v: &[f64]) -> Value {
    Value::Array(v.iter().copied().map(json_num).collect())
}

fn rows3(m: [f64; 9]) -> Value {
    Value::Array(vec![arr_n(&m[0..3]), arr_n(&m[3..6]), arr_n(&m[6..9])])
}

fn rows4(m: [f64; 16]) -> Value {
    Value::Array(vec![
        arr_n(&m[0..4]),
        arr_n(&m[4..8]),
        arr_n(&m[8..12]),
        arr_n(&m[12..16]),
    ])
}

fn transposed3(m: [f64; 9]) -> [f64; 9] {
    [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]]
}

fn transposed4(m: [f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[c * 4 + r] = m[r * 4 + c];
        }
    }
    out
}

/// Shepperd method; flip so `w >= 0` (C++ `rotation_to_quaternion`).
pub fn rotation_to_quaternion(r: Mat3) -> Result<Quat, DomainError> {
    let m = r.row_major();
    let tr = m[0] + m[4] + m[8];
    let (mut w, mut x, mut y, mut z) = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        (
            0.25 * s,
            (m[7] - m[5]) / s,
            (m[2] - m[6]) / s,
            (m[3] - m[1]) / s,
        )
    } else if m[0] > m[4] && m[0] > m[8] {
        let s = (1.0 + m[0] - m[4] - m[8]).sqrt() * 2.0;
        (
            (m[7] - m[5]) / s,
            0.25 * s,
            (m[1] + m[3]) / s,
            (m[2] + m[6]) / s,
        )
    } else if m[4] > m[8] {
        let s = (1.0 + m[4] - m[0] - m[8]).sqrt() * 2.0;
        (
            (m[2] - m[6]) / s,
            (m[1] + m[3]) / s,
            0.25 * s,
            (m[5] + m[7]) / s,
        )
    } else {
        let s = (1.0 + m[8] - m[0] - m[4]).sqrt() * 2.0;
        (
            (m[3] - m[1]) / s,
            (m[2] + m[6]) / s,
            (m[5] + m[7]) / s,
            0.25 * s,
        )
    };
    let n = (w * w + x * x + y * y + z * z).sqrt();
    if !n.is_finite() || n == 0.0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "rotation",
            "nonfinite quaternion from rotation",
        ));
    }
    w /= n;
    x /= n;
    y /= n;
    z /= n;
    if w < 0.0 {
        w = -w;
        x = -x;
        y = -y;
        z = -z;
    }
    Quat::try_from_wxyz(w, x, y, z)
}

fn axis_index(c: u8) -> Option<usize> {
    match c {
        b'x' | b'X' => Some(0),
        b'y' | b'Y' => Some(1),
        b'z' | b'Z' => Some(2),
        _ => None,
    }
}

fn rotation_to_euler_intrinsic(r: Mat3, order: &str) -> [f64; 3] {
    let m = r.row_major();
    let bytes = order.as_bytes();
    let i = axis_index(bytes[0]).unwrap_or(0);
    let j = axis_index(bytes[1]).unwrap_or(1);
    let k = axis_index(bytes[2]).unwrap_or(2);
    let e = if j == (i + 1) % 3 { 1.0 } else { -1.0 };
    let at = |row: usize, col: usize| m[row * 3 + col];
    let sb = (e * at(i, k)).clamp(-1.0, 1.0);
    let mut out = [0.0; 3];
    out[1] = sb.asin();
    if sb.abs() < 1.0 - 1e-12 {
        out[0] = (-e * at(j, k)).atan2(at(k, k));
        out[2] = (-e * at(i, j)).atan2(at(i, i));
    } else {
        out[0] = (sb * at(j, i)).atan2(at(j, j));
        out[2] = 0.0;
    }
    out
}

fn euler_entry(a: [f64; 3]) -> Value {
    let deg = [a[0] * DEG, a[1] * DEG, a[2] * DEG];
    let mut o = Map::new();
    o.insert("radians".into(), arr3(a));
    o.insert("degrees".into(), arr3(deg));
    Value::Object(o)
}

fn euler_block(r: Mat3) -> Value {
    let mut intrinsic = Map::new();
    for order in EULER_ORDERS {
        intrinsic.insert(
            order.into(),
            euler_entry(rotation_to_euler_intrinsic(r, order)),
        );
    }
    let mut extrinsic = Map::new();
    for order in EULER_ORDERS {
        let rev: String = order.chars().rev().collect();
        let a = rotation_to_euler_intrinsic(r, &rev);
        let b = [a[2], a[1], a[0]];
        extrinsic.insert(order.into(), euler_entry(b));
    }
    let mut o = Map::new();
    o.insert("intrinsic".into(), Value::Object(intrinsic));
    o.insert("extrinsic".into(), Value::Object(extrinsic));
    Value::Object(o)
}

fn similarity_object(t: SceneTransform) -> Result<Value, DomainError> {
    let r = t.rotation();
    let rm = r.row_major();
    let q = rotation_to_quaternion(r)?;
    let qw = q.wxyz();
    let qx = q.xyzw();
    let sn = (qw[1] * qw[1] + qw[2] * qw[2] + qw[3] * qw[3]).sqrt();
    let angle = 2.0 * sn.atan2(qw[0]);
    let mut axis = [0.0, 0.0, 1.0];
    if sn > 1e-15 {
        axis = [qw[1] / sn, qw[2] / sn, qw[3] / sn];
    }
    let rotvec = [axis[0] * angle, axis[1] * angle, axis[2] * angle];
    let t_pre = t.translation_before_rotation()?.to_array();
    let mut m4 = [0.0; 16];
    for row in 0..3 {
        for col in 0..3 {
            m4[row * 4 + col] = t.scale() * rm[row * 3 + col];
        }
        m4[row * 4 + 3] = t.translation().to_array()[row];
    }
    m4[15] = 1.0;

    let mut axis_angle = Map::new();
    axis_angle.insert("axis".into(), arr3(axis));
    axis_angle.insert("angle_rad".into(), json_num(angle));
    axis_angle.insert("angle_deg".into(), json_num(angle * DEG));

    let mut rot = Map::new();
    rot.insert("matrix_3x3".into(), rows3(rm));
    rot.insert("matrix_3x3_flat_row_major".into(), arr_n(&rm));
    rot.insert(
        "matrix_3x3_flat_column_major".into(),
        arr_n(&transposed3(rm)),
    );
    rot.insert("quaternion_wxyz".into(), arr_n(&qw));
    rot.insert("quaternion_xyzw".into(), arr_n(&qx));
    rot.insert("axis_angle".into(), Value::Object(axis_angle));
    rot.insert("rotation_vector".into(), arr3(rotvec));
    rot.insert("euler".into(), euler_block(r));

    let mut o = Map::new();
    o.insert("scale".into(), json_num(t.scale()));
    o.insert("translation".into(), arr3(t.translation().to_array()));
    o.insert("translation_before_rotation".into(), arr3(t_pre));
    o.insert("rotation".into(), Value::Object(rot));
    o.insert("matrix_4x4".into(), rows4(m4));
    o.insert("matrix_4x4_flat_row_major".into(), arr_n(&m4));
    o.insert(
        "matrix_4x4_flat_column_major".into(),
        arr_n(&transposed4(m4)),
    );
    o.insert("matrix_3x4_flat_row_major".into(), arr_n(&m4[..12]));
    Ok(Value::Object(o))
}

/// Emit stable JSON object for `doc` (pretty-printed by [`write_scene_transform_json`]).
pub fn scene_transform_to_value(doc: &SceneTransformDoc) -> Result<Value, DomainError> {
    let inv = doc.train_from_world.inverse()?;
    let mut root = Map::new();
    root.insert(
        "format".into(),
        Value::String(SCENE_TRANSFORM_FORMAT.into()),
    );
    root.insert(
        "version".into(),
        Value::Number(Number::from(SCENE_TRANSFORM_VERSION)),
    );
    root.insert(
        "readme".into(),
        Value::Array(README.iter().map(|s| Value::String((*s).into())).collect()),
    );
    let mut centering = Map::new();
    centering.insert("mode".into(), Value::String(doc.center_mode.clone()));
    centering.insert("center_world".into(), arr3(doc.center_world));
    root.insert("centering".into(), Value::Object(centering));
    if let Some(geo) = &doc.georeference {
        let mut g = Map::new();
        g.insert("frame".into(), Value::String("enu".into()));
        g.insert("origin_lat".into(), json_num(geo.origin_lat));
        g.insert("origin_lon".into(), json_num(geo.origin_lon));
        if let Some(alt) = geo.origin_alt {
            g.insert("origin_alt".into(), json_num(alt));
        }
        if let Some(src) = &geo.alt_source {
            g.insert("alt_source".into(), Value::String(src.clone()));
        }
        root.insert("georeference".into(), Value::Object(g));
    }
    root.insert(
        "train_from_world".into(),
        similarity_object(doc.train_from_world)?,
    );
    root.insert("world_from_train".into(), similarity_object(inv)?);
    Ok(Value::Object(root))
}

pub fn scene_transform_to_json(doc: &SceneTransformDoc) -> Result<String, DomainError> {
    let v = scene_transform_to_value(doc)?;
    serde_json::to_string_pretty(&v).map_err(|e| {
        err(
            ErrorCode::WriteFailed,
            "scene_transform",
            format!("JSON encode failed: {e}"),
        )
    })
}

fn read_f64_arr(v: &Value, n: usize, field: &str) -> Result<Vec<f64>, DomainError> {
    let a = v.as_array().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} must be an array"),
        )
    })?;
    if a.len() != n {
        return Err(err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} length {} != {n}", a.len()),
        ));
    }
    let mut out = Vec::with_capacity(n);
    for (i, cell) in a.iter().enumerate() {
        let x = cell.as_f64().ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                field,
                format!("{field}[{i}] non-numeric"),
            )
        })?;
        if !x.is_finite() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                field,
                format!("{field}[{i}] nonfinite"),
            ));
        }
        out.push(x);
    }
    Ok(out)
}

fn parse_similarity(obj: &Value, field: &str) -> Result<SceneTransform, DomainError> {
    let o = obj.as_object().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} must be an object"),
        )
    })?;
    let scale = o
        .get("scale")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| err(ErrorCode::MalformedRecord, "scale", "missing scale"))?;
    if !(scale.is_finite() && scale > 0.0) {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "scale",
            format!("scale must be positive finite, got {scale}"),
        ));
    }
    let rot = o
        .get("rotation")
        .ok_or_else(|| err(ErrorCode::MalformedRecord, "rotation", "missing rotation"))?;
    let flat = rot.get("matrix_3x3_flat_row_major").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "matrix_3x3_flat_row_major",
            "missing matrix_3x3_flat_row_major",
        )
    })?;
    let rm = read_f64_arr(flat, 9, "matrix_3x3_flat_row_major")?;
    let mut m = [0.0; 9];
    m.copy_from_slice(&rm);
    let r = Mat3::try_from_row_major(m)?;
    let t_arr = o.get("translation").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "translation",
            "missing translation",
        )
    })?;
    let tv = read_f64_arr(t_arr, 3, "translation")?;
    let t = Vec3::try_new(tv[0], tv[1], tv[2])?;
    SceneTransform::try_new(r, t, scale)
}

/// Parse document bytes. Rejects wrong format/version and nonfinite inputs.
pub fn parse_scene_transform_bytes(bytes: &[u8]) -> Result<SceneTransformDoc, DomainError> {
    let root: Value = serde_json::from_slice(bytes).map_err(|e| {
        err(
            ErrorCode::MalformedRecord,
            "scene_transform",
            format!("JSON parse failed: {e}"),
        )
    })?;
    let obj = root.as_object().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "scene_transform",
            "root must be an object",
        )
    })?;
    let format = obj.get("format").and_then(|v| v.as_str()).unwrap_or("");
    if format != SCENE_TRANSFORM_FORMAT {
        return Err(err(
            ErrorCode::UnsupportedFormat,
            "format",
            format!("expected {SCENE_TRANSFORM_FORMAT}, got '{format}'"),
        ));
    }
    let version = obj.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
    if version != SCENE_TRANSFORM_VERSION {
        return Err(err(
            ErrorCode::UnsupportedFormat,
            "version",
            format!("unsupported scene-transform version {version}"),
        ));
    }
    let train = obj.get("train_from_world").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "train_from_world",
            "missing train_from_world",
        )
    })?;
    let train_from_world = parse_similarity(train, "train_from_world")?;
    // Optional consistency: world_from_train inverse within tolerance.
    if let Some(wft) = obj.get("world_from_train") {
        let inv = parse_similarity(wft, "world_from_train")?;
        let expect = train_from_world.inverse()?;
        let ds = (inv.scale() - expect.scale()).abs();
        if ds > 1e-9 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "world_from_train.scale",
                "world_from_train is not the inverse of train_from_world",
            ));
        }
    }
    let centering = obj
        .get("centering")
        .ok_or_else(|| err(ErrorCode::MalformedRecord, "centering", "missing centering"))?;
    let mode = centering
        .get("mode")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(ErrorCode::MalformedRecord, "centering.mode", "missing mode"))?
        .to_owned();
    let cw = read_f64_arr(
        centering.get("center_world").ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "center_world",
                "missing center_world",
            )
        })?,
        3,
        "center_world",
    )?;
    let mut center_world = [0.0; 3];
    center_world.copy_from_slice(&cw);

    let georeference = if let Some(g) = obj.get("georeference") {
        let lat = g
            .get("origin_lat")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "origin_lat",
                    "georeference missing origin_lat",
                )
            })?;
        let lon = g
            .get("origin_lon")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "origin_lon",
                    "georeference missing origin_lon",
                )
            })?;
        if !lat.is_finite() || !lon.is_finite() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "georeference",
                "nonfinite origin_lat/lon",
            ));
        }
        let origin_alt = match g.get("origin_alt") {
            Some(v) => {
                let a = v.as_f64().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "origin_alt",
                        "origin_alt non-numeric",
                    )
                })?;
                if !a.is_finite() {
                    return Err(err(
                        ErrorCode::ValueOutOfRange,
                        "origin_alt",
                        "nonfinite origin_alt",
                    ));
                }
                Some(a)
            }
            None => None,
        };
        let alt_source = g
            .get("alt_source")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        Some(GeoReference {
            origin_lat: lat,
            origin_lon: lon,
            origin_alt,
            alt_source,
        })
    } else {
        None
    };

    Ok(SceneTransformDoc {
        train_from_world,
        center_mode: mode,
        center_world,
        georeference,
    })
}

pub fn read_scene_transform_json(path: &Path) -> Result<SceneTransformDoc, DomainError> {
    let bytes = std::fs::read(path).map_err(|e| {
        DomainError::try_new(ErrorCode::ReadFailed, format!("read failed: {e}"))
            .expect("message")
            .with_context(ContextKey::Path, path.display().to_string())
            .expect("path")
    })?;
    parse_scene_transform_bytes(&bytes)
}

/// Atomic write: validate encode first, leave destination untouched on failure.
pub fn write_scene_transform_json(
    path: &Path,
    doc: &SceneTransformDoc,
    cancel: Option<&CancelToken>,
) -> Result<(), DomainError> {
    let text = scene_transform_to_json(doc)?;
    let bytes = text.into_bytes();
    // Round-trip validate before touching the destination.
    let _ = parse_scene_transform_bytes(&bytes)?;
    let mut options = PublishOptions::default();
    if let Some(token) = cancel {
        options = options.with_cancel(token);
    }
    publish_bytes_validated(path, &bytes, options, |payload| {
        parse_scene_transform_bytes(payload).map(|_| ())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/scene/scene_transform.json")
    }

    #[test]
    fn fixture_roundtrip_active_transform() {
        let doc = read_scene_transform_json(&fixture()).unwrap();
        assert_eq!(doc.center_mode, "point-median");
        assert_eq!(doc.center_world, [4.0, 3.0, -0.5]);
        assert!((doc.train_from_world.scale() - 0.5).abs() < 1e-15);
        let inv = doc.train_from_world.inverse().unwrap();
        let id = doc
            .train_from_world
            .apply(inv.apply(Vec3::try_new(1.0, 2.0, 3.0).unwrap()).unwrap())
            .unwrap();
        assert!((id.x() - 1.0).abs() < 1e-12);
        let p = Vec3::try_new(1.0, 2.0, 3.0).unwrap();
        let train = doc.train_from_world.apply(p).unwrap();
        assert!((train.x() - 0.5).abs() < 1e-12);
        assert!((train.y() + 1.5).abs() < 1e-12);
        assert!((train.z() - 1.75).abs() < 1e-12);
        let geo = doc.georeference.as_ref().unwrap();
        assert!((geo.origin_lat - 47.6205).abs() < 1e-12);
    }

    #[test]
    fn reject_nonfinite_translation() {
        let bad = br#"{
          "format":"spirula-scene-transform","version":1,
          "centering":{"mode":"none","center_world":[0,0,0]},
          "train_from_world":{
            "scale":1.0,
            "translation":[0,0,"nan"],
            "rotation":{"matrix_3x3_flat_row_major":[1,0,0,0,1,0,0,0,1]}
          }
        }"#;
        // nan as string fails numeric parse
        assert!(parse_scene_transform_bytes(bad).is_err());
        let bad2 = br#"{
          "format":"spirula-scene-transform","version":1,
          "centering":{"mode":"none","center_world":[0,0,0]},
          "train_from_world":{
            "scale":1.0,
            "translation":[0,0,null],
            "rotation":{"matrix_3x3_flat_row_major":[1,0,0,0,1,0,0,0,1]}
          }
        }"#;
        assert!(parse_scene_transform_bytes(bad2).is_err());
    }

    #[test]
    fn reject_nonpositive_scale() {
        let bad = br#"{
          "format":"spirula-scene-transform","version":1,
          "centering":{"mode":"none","center_world":[0,0,0]},
          "train_from_world":{
            "scale":0.0,
            "translation":[0,0,0],
            "rotation":{"matrix_3x3_flat_row_major":[1,0,0,0,1,0,0,0,1]}
          }
        }"#;
        assert!(parse_scene_transform_bytes(bad).is_err());
    }

    #[test]
    fn emit_inverse_roundtrip() {
        let doc = read_scene_transform_json(&fixture()).unwrap();
        let json = scene_transform_to_json(&doc).unwrap();
        let back = parse_scene_transform_bytes(json.as_bytes()).unwrap();
        assert_eq!(back.center_mode, doc.center_mode);
        assert_eq!(back.center_world, doc.center_world);
        assert!((back.train_from_world.scale() - doc.train_from_world.scale()).abs() < 1e-15);
        let a = doc.train_from_world.rotation().row_major();
        let b = back.train_from_world.rotation().row_major();
        for i in 0..9 {
            assert!((a[i] - b[i]).abs() < 1e-12);
        }
    }
}
