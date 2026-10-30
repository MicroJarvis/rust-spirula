//! Metashape camera-export XML + PLY adapter (P3-F04).
//!
//! Primary path: camera-export `.xml` (+ optional `.ply`). `.psx` projects are
//! detected for probe/auto-detect but load as typed unsupported without a zip
//! runtime (no miniz / zip crate).
//!
//! Pin: `MetashapeParser.cpp` @ `9a24f099…`.

mod xml;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use spirula_core::{CancelToken, ErrorCode};

use crate::colmap::ColmapLoadConfig;
use crate::colmap::DatasetFormat;
use crate::colmap::ParsedDataset;
use crate::colmap::budget::{
    MAX_COLMAP_FILE_BYTES, MAX_LOAD_MEMORY_BYTES, ParseCancel, ParseMem, err, err_path,
    prepare_vec_push,
};
use crate::nerfstudio::{estimate_json_bytes, load_nerfstudio_meta_value_mem};

/// Conservative pre-charge for one Metashape sensor `serde_json::Value` (keys,
/// numbers, model/distortion strings, and map capacity slack) **before** `json!`.
const SENSOR_VALUE_PRECHARGE: u64 = 4096;
/// Root `{"frames":[…]}` wrapper (frame Values already charged at clone).
const META_ROOT_PRECHARGE: u64 = 128;

/// Bytes for `json!([[f64;4];4])` array **buffers** alone on 64-bit:
/// 1 outer + 4 row `Vec<Value>`, each capacity ≥4 → `5 * 4 * size_of::<Value>()`
/// (= 640 when `Value` is 32 bytes). A `512` constant cannot cover this floor.
pub(crate) fn transform_matrix_array_slots_bytes() -> u64 {
    let value = std::mem::size_of::<Value>() as u64;
    const ARRAYS: u64 = 5;
    const CAP_PER: u64 = 4;
    ARRAYS.saturating_mul(CAP_PER).saturating_mul(value)
}

/// Defensible pre-charge **before** `json!` builds a 4×4 `transform_matrix`.
///
/// Must stay **strictly above** [`transform_matrix_array_slots_bytes`] (and the
/// false historical `512`). `4096` covers the 640-byte slot floor, 2× allocator
/// size-class slack, and Number/tag scratch without under-bounding peak.
pub(crate) const TRANSFORM_MATRIX_PRECHARGE: u64 = 4096;

const _: () = {
    assert!(TRANSFORM_MATRIX_PRECHARGE > 512);
    // 5*4*32 = 640 on typical 64-bit Value layout; keep headroom even if Value grows.
    assert!(TRANSFORM_MATRIX_PRECHARGE >= 5 * 4 * 32);
};

/// Pre-charge **before** `frame["transform_matrix"] = tm` mutates the object.
///
/// Key `String` + one map entry (`String` header + `Value` slot) + growth slack
/// for `serde_json::Map` / IndexMap reallocation on insert.
pub(crate) fn transform_matrix_map_insert_precharge(key: &str) -> u64 {
    let value = std::mem::size_of::<Value>() as u64;
    let key_bytes = (key.len() as u64).saturating_add(24); // String heap + header
    let entry = value.saturating_add(64); // Value slot + map node slack
    key_bytes.saturating_add(entry).saturating_add(256) // map Vec/(IndexMap) capacity doubling slack
}

use self::xml::{XmlNode, parse_xml_bytes};

const READ_CHUNK: usize = 256 * 1024;

fn open_err(path: &Path, e: std::io::Error) -> spirula_core::DomainError {
    let code = if e.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::NotFound
    } else if e.kind() == std::io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::ReadFailed
    };
    err_path(
        code,
        &path.display().to_string(),
        format!("cannot open: {e}"),
    )
}

fn read_capped(
    path: &Path,
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>, spirula_core::DomainError> {
    let meta = std::fs::metadata(path).map_err(|e| open_err(path, e))?;
    let len = meta.len();
    if len > MAX_COLMAP_FILE_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "file_size",
            format!(
                "Metashape file {} exceeds budget {MAX_COLMAP_FILE_BYTES} bytes",
                path.display()
            ),
        ));
    }
    mem.charge(len, "metashape_file")?;
    let mut charged = len;
    let mut file = std::fs::File::open(path).map_err(|e| open_err(path, e))?;
    let mut data = Vec::new();
    if len > 0 {
        data.try_reserve(len as usize).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "metashape_file",
                format!("reservation of {len} bytes failed"),
            )
        })?;
    }
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;
    let mut buf = [0u8; READ_CHUNK];
    let mut read_total = 0u64;
    loop {
        use std::io::Read;
        let n = file.read(&mut buf).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &path.display().to_string(),
                format!("cannot read: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        read_total = read_total.saturating_add(n as u64);
        if read_total > MAX_COLMAP_FILE_BYTES {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "file_size",
                "Metashape file grew past budget during read",
            ));
        }
        if read_total > charged {
            mem.charge(read_total - charged, "metashape_file")?;
            charged = read_total;
        }
        data.extend_from_slice(&buf[..n]);
        cp.tick(n)?;
    }
    Ok(data)
}

fn ends_with_ci(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext.trim_start_matches('.')))
}

type MetashapeInputs = (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>);

/// Locate camera-export `.xml` / `.psx` / `.ply` under `dataset_dir`.
pub fn find_metashape_inputs(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
) -> Result<MetashapeInputs, spirula_core::DomainError> {
    let resolve = |hint: &Option<String>, ext: &str| -> Option<PathBuf> {
        if let Some(h) = hint.as_ref().filter(|s| !s.is_empty()) {
            let p = if Path::new(h).is_absolute() {
                PathBuf::from(h)
            } else {
                dataset_dir.join(h)
            };
            return Some(p);
        }
        let Ok(rd) = std::fs::read_dir(dataset_dir) else {
            return None;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_file() && ends_with_ci(&p, ext) {
                return Some(p);
            }
        }
        None
    };

    let xml = resolve(&cfg.metashape_xml, "xml");
    let psx = resolve(&cfg.metashape_psx, "psx");
    let ply = resolve(&cfg.metashape_ply, "ply");
    Ok((xml, psx, ply))
}

fn child_double(node: &XmlNode, name: &str) -> f64 {
    node.find(name)
        .map(|n| n.text.trim().parse().unwrap_or(0.0))
        .unwrap_or(0.0)
}

fn attr_double(node: &XmlNode, name: &str) -> f64 {
    node.attr(name).and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

fn split_doubles(s: &str) -> Vec<f64> {
    s.split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect()
}

fn norm_slashes(s: &str) -> String {
    s.replace('\\', "/")
}

/// Longest-prefix substring match over image names (pin Metashape `match_substring`).
/// Iterates by Unicode scalar count so multi-byte labels never panic on byte cuts.
fn match_substring(names: &[String], s: &str) -> Vec<String> {
    let q = norm_slashes(s);
    let chars: Vec<char> = q.chars().collect();
    for len in (1..=chars.len()).rev() {
        let needle: String = chars[..len].iter().collect();
        let matches: Vec<String> = names
            .iter()
            .filter(|n| n.contains(&needle))
            .cloned()
            .collect();
        if !matches.is_empty() {
            return matches;
        }
    }
    Vec::new()
}

const MAX_IMAGE_WALK_DEPTH: u32 = 64;

fn list_image_filenames(
    root: &Path,
    image_dir: &Path,
    cancel: Option<&CancelToken>,
    mem: &mut ParseMem,
) -> Result<Vec<String>, spirula_core::DomainError> {
    let mut out = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut cp = ParseCancel::new(cancel);
    walk_dir(image_dir, root, &mut out, &mut visited, 0, &mut cp, mem)?;
    Ok(out)
}

fn charge_visited_path(mem: &mut ParseMem, key: &Path) -> Result<(), spirula_core::DomainError> {
    mem.charge_btree_entries::<PathBuf>(1, "image_walk_visited")?;
    mem.charge_str_bytes(key.as_os_str().len(), "image_walk_visited")
}

fn push_image_filename(
    out: &mut Vec<String>,
    s: String,
    mem: &mut ParseMem,
) -> Result<(), spirula_core::DomainError> {
    prepare_vec_push(out, mem, "image_filenames")?;
    mem.charge_str_bytes(s.len(), "image_filenames")?;
    out.push(s);
    Ok(())
}

fn walk_dir(
    dir: &Path,
    root: &Path,
    out: &mut Vec<String>,
    visited: &mut std::collections::BTreeSet<PathBuf>,
    depth: u32,
    cp: &mut ParseCancel<'_>,
    mem: &mut ParseMem,
) -> Result<(), spirula_core::DomainError> {
    if depth > MAX_IMAGE_WALK_DEPTH {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "image_dir",
            format!("MetashapeParser: image_dir walk exceeds depth {MAX_IMAGE_WALK_DEPTH}"),
        ));
    }
    cp.tick(1)?;
    // Do not follow symlink cycles: canonicalize when possible; skip repeats.
    let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if visited.contains(&key) {
        return Ok(());
    }
    charge_visited_path(mem, &key)?;
    visited.insert(key);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for ent in rd.flatten() {
        cp.tick(1)?;
        let p = ent.path();
        let meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            // Resolve once; cycle detection via visited canonicalize.
            if p.is_dir() {
                walk_dir(&p, root, out, visited, depth + 1, cp, mem)?;
            } else if p.is_file()
                && let Ok(rel) = p.strip_prefix(root)
            {
                let s = rel.to_string_lossy().replace('\\', "/");
                push_image_filename(out, s, mem)?;
            }
            continue;
        }
        if meta.is_dir() {
            walk_dir(&p, root, out, visited, depth + 1, cp, mem)?;
        } else if meta.is_file()
            && let Ok(rel) = p.strip_prefix(root)
        {
            let s = rel.to_string_lossy().replace('\\', "/");
            push_image_filename(out, s, mem)?;
        }
    }
    Ok(())
}

fn mat4_mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for r in 0..4 {
        for c in 0..4 {
            let mut v = 0.0;
            for k in 0..4 {
                v += a[r * 4 + k] * b[k * 4 + c];
            }
            out[r * 4 + c] = v;
        }
    }
    out
}

fn identity4() -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// Build transforms.json-shaped meta from a Metashape camera-export XML.
pub(crate) fn metashape_meta_from_xml(
    dataset_dir: &Path,
    xml_path: &Path,
    ply_path: Option<&Path>,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
    mem: &mut ParseMem,
) -> Result<Value, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;
    let bytes = read_capped(xml_path, mem, cancel)?;
    cp.poll_now()?;
    let doc = parse_xml_bytes(&bytes, mem, cancel)?;
    // C++ `xml_parse_file` yields the root element; exports are
    // `<document><chunk>…` so prefer `document/chunk`, else first child.
    let chunk = {
        let top = doc.children.first().ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "metashape_xml",
                format!("MetashapeParser: empty document in {}", xml_path.display()),
            )
        })?;
        if top.name == "document" {
            top.find("chunk")
                .or_else(|| top.children.first())
                .ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "metashape_xml",
                        format!("MetashapeParser: no <chunk> in {}", xml_path.display()),
                    )
                })?
        } else if top.name == "chunk" {
            top
        } else {
            top.find("chunk").unwrap_or(top)
        }
    };

    let sensors = chunk.find("sensors").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "sensors",
            "MetashapeParser: no sensors found",
        )
    })?;

    let mut sensor_dict: BTreeMap<String, Value> = BTreeMap::new();
    for sensor in sensors.iter("sensor") {
        cp.tick(1)?;
        let Some(id) = sensor.attr("id") else {
            continue;
        };
        let type_s = sensor.attr("type").unwrap_or("");
        let intrinsic_free = type_s == "spherical" || type_s == "cylindrical";
        if !intrinsic_free && sensor.find("calibration").is_none() {
            continue;
        }
        let camera_model = match type_s {
            "frame" => "PINHOLE",
            "fisheye" | "equidistant_fisheye" => "FISHEYE",
            "equisolid_fisheye" => "EQUISOLID",
            "spherical" => "EQUIRECTANGULAR",
            other => {
                return Err(err(
                    ErrorCode::UnsupportedFormat,
                    "sensor_type",
                    format!("MetashapeParser: unsupported Metashape sensor type '{other}'"),
                ));
            }
        };
        let resolution = sensor.find("resolution").ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "resolution",
                format!("MetashapeParser: sensor {id} has no resolution"),
            )
        })?;
        let w = attr_double(resolution, "width");
        let h = attr_double(resolution, "height");

        let mut calib = None;
        for c in sensor.findall("calibration") {
            if let Some(cls) = c.attr("class")
                && cls != "initial"
            {
                calib = Some(c);
                break;
            }
        }

        // Charge map entry + sensor Value **before** json! / insert.
        mem.charge_btree_entries::<(String, Value)>(1, "metashape_sensors")?;
        mem.charge_str_bytes(id.len(), "metashape_sensors")?;
        mem.charge(SENSOR_VALUE_PRECHARGE, "metashape_sensors")?;

        let mut s = json!({
            "camera_model": camera_model,
            "w": w,
            "h": h,
        });
        if let Some(calib) = calib {
            let f_node = calib.find("f").ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "f",
                    format!("MetashapeParser: focal length not found for sensor {id}"),
                )
            })?;
            let fl: f64 = f_node.text.trim().parse().map_err(|_| {
                err(
                    ErrorCode::MalformedRecord,
                    "f",
                    format!("MetashapeParser: bad focal length for sensor {id}"),
                )
            })?;
            if !fl.is_finite() || fl == 0.0 {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "f",
                    format!("MetashapeParser: non-finite/zero focal for sensor {id}"),
                ));
            }
            let p3 = child_double(calib, "p3");
            let p4 = child_double(calib, "p4");
            if p3 != 0.0 || p4 != 0.0 {
                return Err(err(
                    ErrorCode::UnsupportedFormat,
                    "p3_p4",
                    format!(
                        "MetashapeParser: sensor {id} has nonzero p3/p4 (pre-1.6 tangential scaling)"
                    ),
                ));
            }
            // p1/p2 swap; b1/b2 ÷ f; k4 is 4th radial → THIN_PRISM hint.
            s["fl_x"] = json!(fl);
            s["fl_y"] = json!(fl);
            s["cx"] = json!(child_double(calib, "cx") + w / 2.0);
            s["cy"] = json!(child_double(calib, "cy") + h / 2.0);
            s["k1"] = json!(child_double(calib, "k1"));
            s["k2"] = json!(child_double(calib, "k2"));
            s["k3"] = json!(child_double(calib, "k3"));
            s["k4"] = json!(child_double(calib, "k4"));
            s["p1"] = json!(child_double(calib, "p2"));
            s["p2"] = json!(child_double(calib, "p1"));
            s["b1"] = json!(child_double(calib, "b1") / fl);
            s["b2"] = json!(child_double(calib, "b2") / fl);
            s["camera_distortion"] = json!("THIN_PRISM");
        } else if intrinsic_free {
            s["fl_x"] = json!(w / 2.0);
            s["fl_y"] = json!(h);
            s["cx"] = json!(w / 2.0);
            s["cy"] = json!(h / 2.0);
        } else {
            return Err(err(
                ErrorCode::MalformedRecord,
                "calibration",
                format!("MetashapeParser: sensor {id} has no adjusted calibration"),
            ));
        }
        let actual = estimate_json_bytes(&s);
        if actual > SENSOR_VALUE_PRECHARGE {
            mem.charge(actual - SENSOR_VALUE_PRECHARGE, "metashape_sensors")?;
        }
        sensor_dict.insert(id.to_owned(), s);
    }

    let mut component_transforms: BTreeMap<String, [f64; 16]> = BTreeMap::new();
    let mut component_groups: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(components) = chunk.find("components") {
        for component in components.iter("component") {
            cp.tick(1)?;
            let cid = component.attr("id").unwrap_or("").to_owned();
            if let Some(transform) = component.find("transform") {
                let mut m = identity4();
                let mut scale = 1.0;
                if let Some(sc) = transform.find("scale") {
                    scale = sc.text.trim().parse().unwrap_or(1.0);
                }
                if let Some(rotation) = transform.find("rotation") {
                    let r = split_doubles(&rotation.text);
                    if r.len() >= 9 {
                        for i in 0..3 {
                            for j in 0..3 {
                                m[i * 4 + j] = r[i * 3 + j];
                            }
                        }
                    }
                }
                for i in 0..3 {
                    for j in 0..3 {
                        m[i * 4 + j] *= scale;
                    }
                }
                if let Some(translation) = transform.find("translation") {
                    let t = split_doubles(&translation.text);
                    if t.len() >= 3 {
                        for i in 0..3 {
                            m[i * 4 + 3] = t[i];
                        }
                    }
                }
                mem.charge_btree_entries::<(String, [f64; 16])>(1, "metashape_components")?;
                mem.charge_str_bytes(cid.len(), "metashape_components")?;
                component_transforms.insert(cid.clone(), m);
            }
            let mut ids = Vec::new();
            for ci in component.iter("camera_ids") {
                for tok in ci.text.split_whitespace() {
                    prepare_vec_push(&mut ids, mem, "metashape_components")?;
                    mem.charge_str_bytes(tok.len(), "metashape_components")?;
                    ids.push(tok.to_owned());
                }
            }
            if !ids.is_empty() {
                prepare_vec_push(&mut component_groups, mem, "metashape_components")?;
                mem.charge_str_bytes(cid.len(), "metashape_components")?;
                // ids Vec header already charged via push growth; keep group slot.
                component_groups.push((cid, ids));
            }
        }
    }

    let cameras_node = chunk.find("cameras").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "cameras",
            format!(
                "MetashapeParser: cameras not found in {}",
                xml_path.display()
            ),
        )
    })?;
    let all_cameras = cameras_node.iter("camera");
    if !all_cameras.iter().any(|c| c.find("transform").is_some()) {
        return Err(err(
            ErrorCode::MalformedRecord,
            "transform",
            format!(
                "MetashapeParser: no camera in {} is aligned",
                xml_path.display()
            ),
        ));
    }

    let image_dir = dataset_dir.join(&cfg.image_dir);
    if !image_dir.is_dir() && cfg.require_image_files {
        return Err(err_path(
            ErrorCode::NotFound,
            &image_dir.display().to_string(),
            format!(
                "MetashapeParser: image directory {} does not exist",
                image_dir.display()
            ),
        ));
    }
    let image_filenames = if image_dir.is_dir() {
        list_image_filenames(dataset_dir, &image_dir, cancel, mem)?
    } else {
        Vec::new()
    };

    struct ValidCamera {
        transform: [f64; 16],
        component_id: String,
        frame: Value,
    }
    let mut valid_cameras: BTreeMap<String, ValidCamera> = BTreeMap::new();
    let mut valid_order: Vec<String> = Vec::new();

    for camera in all_cameras {
        cp.tick(1)?;
        let Some(cam_id) = camera.attr("id") else {
            continue;
        };
        let label = camera
            .attr("label")
            .map(|s| s.to_owned())
            .unwrap_or_else(|| format!("camera {cam_id}"));

        let matches = if image_filenames.is_empty() && !cfg.require_image_files {
            vec![label.clone()]
        } else {
            match_substring(&image_filenames, &label)
        };
        if matches.len() != 1 {
            continue;
        }
        let Some(sensor_id) = camera.attr("sensor_id") else {
            continue;
        };
        let Some(sensor) = sensor_dict.get(sensor_id) else {
            continue;
        };
        let Some(transform) = camera.find("transform") else {
            continue;
        };
        let t = split_doubles(&transform.text);
        if t.len() < 16 || t.iter().any(|v| !v.is_finite()) {
            continue;
        }
        let mut m = [0.0; 16];
        m.copy_from_slice(&t[..16]);

        let component_id = camera.attr("component_id").unwrap_or("").to_owned();
        // Peak: BTree slot + key + component_id + transform + sensor clone frame.
        mem.charge_btree_entries::<(String, ValidCamera)>(1, "metashape_cameras")?;
        mem.charge_str_bytes(cam_id.len(), "metashape_cameras")?;
        mem.charge_str_bytes(component_id.len(), "metashape_cameras")?;
        mem.charge(estimate_json_bytes(sensor), "metashape_cameras")?;
        mem.charge_str_bytes(matches[0].len(), "metashape_cameras")?;

        let mut frame = sensor.clone();
        frame["file_path"] = json!(matches[0]);
        prepare_vec_push(&mut valid_order, mem, "metashape_cameras")?;
        mem.charge_str_bytes(cam_id.len(), "metashape_cameras")?;
        valid_order.push(cam_id.to_owned());
        valid_cameras.insert(
            cam_id.to_owned(),
            ValidCamera {
                transform: m,
                component_id,
                frame,
            },
        );
    }

    let mut group: Vec<String> = Vec::new();
    if component_groups.is_empty() {
        for id in &valid_order {
            prepare_vec_push(&mut group, mem, "metashape_group")?;
            mem.charge_str_bytes(id.len(), "metashape_group")?;
            group.push(id.clone());
        }
    } else {
        let mut best = 0usize;
        let mut best_count = 0usize;
        mem.charge_elems::<usize>(component_groups.len(), "metashape_group")?;
        let mut counts = vec![0usize; component_groups.len()];
        for (g, (_cid, ids)) in component_groups.iter().enumerate() {
            for id in ids {
                if valid_cameras.contains_key(id) {
                    counts[g] += 1;
                }
            }
            if counts[g] > best_count {
                best_count = counts[g];
                best = g;
            }
        }
        let mut ids = Vec::new();
        for id in &component_groups[best].1 {
            prepare_vec_push(&mut ids, mem, "metashape_group")?;
            mem.charge_str_bytes(id.len(), "metashape_group")?;
            ids.push(id.clone());
        }
        ids.sort();
        for id in &valid_order {
            if ids.binary_search(id).is_ok() {
                prepare_vec_push(&mut group, mem, "metashape_group")?;
                mem.charge_str_bytes(id.len(), "metashape_group")?;
                group.push(id.clone());
            }
        }
    }

    let mut frames = Vec::new();
    for id in &group {
        cp.tick(1)?;
        let vc = valid_cameras.get_mut(id).unwrap();
        let mut m = vc.transform;
        if !vc.component_id.is_empty()
            && let Some(c) = component_transforms.get(&vc.component_id)
        {
            m = mat4_mul(c, &m);
            let det = m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8])
                + m[2] * (m[4] * m[9] - m[5] * m[8]);
            let s = det.cbrt();
            if s != 0.0 {
                for r in 0..3 {
                    for col in 0..3 {
                        m[r * 4 + col] /= s;
                    }
                }
            }
        }
        // OpenCV → OpenGL: negate Y and Z columns.
        for r in 0..4 {
            m[r * 4 + 1] = -m[r * 4 + 1];
            m[r * 4 + 2] = -m[r * 4 + 2];
        }
        // Charge TRANSFORM_MATRIX_PRECHARGE (4096, not 512) *before* `json!`.
        // Floor is 5×4×size_of::<Value>() array slots (≥640); 512 is insufficient.
        debug_assert!(TRANSFORM_MATRIX_PRECHARGE > transform_matrix_array_slots_bytes());
        mem.charge(TRANSFORM_MATRIX_PRECHARGE, "metashape_frames")?;
        let tm = json!([
            [m[0], m[1], m[2], m[3]],
            [m[4], m[5], m[6], m[7]],
            [m[8], m[9], m[10], m[11]],
            [m[12], m[13], m[14], m[15]],
        ]);
        // Charge map/key growth *before* inserting into the frame object.
        const TM_KEY: &str = "transform_matrix";
        mem.charge(
            transform_matrix_map_insert_precharge(TM_KEY),
            "metashape_frames",
        )?;
        vc.frame[TM_KEY] = tm;
        // Clone peak: charge frame estimate before clone; Vec slot via prepare_vec_push
        // (max growth charged before try_reserve — no post-alloc delta).
        prepare_vec_push(&mut frames, mem, "metashape_frames")?;
        mem.charge(estimate_json_bytes(&vc.frame), "metashape_frames")?;
        frames.push(vc.frame.clone());
    }
    if frames.is_empty() {
        return Err(err(
            ErrorCode::MalformedRecord,
            "cameras",
            format!(
                "MetashapeParser: no usable aligned cameras in {}",
                xml_path.display()
            ),
        ));
    }

    // Frame Values already charged; charge only the root object wrapper before
    // moving frames into the final meta (no post-return estimate_json_bytes).
    mem.charge(META_ROOT_PRECHARGE, "metashape_meta")?;
    let mut meta = json!({ "frames": frames });
    if let Some(ply) = ply_path {
        let rel = ply
            .strip_prefix(dataset_dir)
            .unwrap_or(ply)
            .to_string_lossy()
            .replace('\\', "/");
        mem.charge_str_bytes(rel.len().saturating_add(16), "metashape_meta")?;
        meta["ply_file_path"] = json!(rel);
    }
    Ok(meta)
}

/// Load Metashape dataset (XML + optional PLY). `.psx`-only → typed unsupported.
pub fn load_metashape_dataset(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    load_metashape_dataset_budgeted(dataset_dir, cfg, cancel, MAX_LOAD_MEMORY_BYTES)
}

pub fn load_metashape_dataset_budgeted(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
    memory_budget_bytes: u64,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    let mut mem = ParseMem::with_cap(memory_budget_bytes);
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;

    if !dataset_dir.is_dir() {
        return Err(err_path(
            if dataset_dir.exists() {
                ErrorCode::InvalidArgument
            } else {
                ErrorCode::NotFound
            },
            &dataset_dir.display().to_string(),
            if dataset_dir.exists() {
                "dataset path is not a directory"
            } else {
                "dataset path does not exist"
            },
        ));
    }

    let (xml, psx, ply) = find_metashape_inputs(dataset_dir, cfg)?;
    if xml.is_none() && psx.is_some() {
        return Err(err(
            ErrorCode::UnimplementedOption,
            "metashape_psx",
            "Metashape .psx project requires nested zip extraction; spirula-io has no zip runtime — export cameras to .xml (+ .ply) instead",
        )
        .with_context(spirula_core::ContextKey::Format, DatasetFormat::Metashape.as_str())
        .expect("format"));
    }
    let xml_path = xml.ok_or_else(|| {
        err(
            ErrorCode::NotFound,
            "metashape_xml",
            "MetashapeParser: no camera-export .xml or .psx project found",
        )
    })?;
    if !xml_path.is_file() {
        return Err(err_path(
            ErrorCode::NotFound,
            &xml_path.display().to_string(),
            "MetashapeParser: metashape xml not found",
        ));
    }

    let ply_ref = ply.as_deref().filter(|p| p.is_file());
    // Conversion charges sensor/component/camera/frame/walk peak **before** each
    // allocate inside `metashape_meta_from_xml` (no post-return estimate).
    let meta = metashape_meta_from_xml(dataset_dir, &xml_path, ply_ref, cfg, cancel, &mut mem)?;
    cp.poll_now()?;

    let mut loaded = load_nerfstudio_meta_value_mem(&meta, dataset_dir, cfg, cancel, &mut mem)?;
    loaded.format = DatasetFormat::Metashape;
    loaded.recon_dir = xml_path.clone();
    loaded.summary.format = DatasetFormat::Metashape.as_str().into();
    loaded.summary.recon_dir = xml_path.display().to_string();
    Ok(loaded)
}

/// Reason string for probe when only `.psx` markers are present.
pub fn psx_unsupported_reason() -> &'static str {
    "Metashape .psx detected; nested zip project load needs a zip runtime (use camera-export .xml + .ply)"
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::ErrorCode;

    #[test]
    fn transform_matrix_precharge_covers_json_array_capacity() {
        // Independent check: 5 Vecs × capacity 4 × size_of::<Value>() = 640.
        let floor = transform_matrix_array_slots_bytes();
        assert!(floor >= 640, "floor for 64-bit Value layout, got {floor}");
        let tm = json!([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let mut slots = 0usize;
        let Value::Array(outer) = &tm else {
            panic!("expected array");
        };
        slots += outer.capacity();
        for row in outer {
            let Value::Array(r) = row else {
                panic!("expected row array");
            };
            slots += r.capacity();
        }
        let array_only = (slots as u64) * (std::mem::size_of::<Value>() as u64);
        assert!(
            array_only >= floor,
            "observed array_only={array_only} floor={floor}"
        );
        assert!(
            TRANSFORM_MATRIX_PRECHARGE >= array_only,
            "precharge {} must cover observed array slots {}",
            TRANSFORM_MATRIX_PRECHARGE,
            array_only
        );
        // Runtime: false 512 cannot cover measured peak.
        assert!(array_only > 512);
    }

    #[test]
    fn transform_matrix_budget_512_insufficient_for_matrix_peak() {
        // Regression: a budget that only covers the false 512 constant must
        // reject, because the matrix peak (array slots alone) exceeds 512.
        let peak = transform_matrix_array_slots_bytes();
        assert!(peak > 512, "peak={peak} must exceed false 512 bound");
        assert!(
            TRANSFORM_MATRIX_PRECHARGE >= peak,
            "4096 must cover peak {peak}"
        );

        let mut mem512 = ParseMem::with_cap(512);
        let e = mem512
            .charge(TRANSFORM_MATRIX_PRECHARGE, "metashape_frames")
            .expect_err("512-cap cannot fund TRANSFORM_MATRIX_PRECHARGE");
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);

        // Even charging only the measured array-slot floor must fail under 512.
        let mut mem512b = ParseMem::with_cap(512);
        let e2 = mem512b
            .charge(peak, "metashape_frames")
            .expect_err("512-cap cannot fund array-slot peak");
        assert_eq!(e2.code(), ErrorCode::ValueOutOfRange);

        // Real precharge fits under its own cap (sanity).
        let mut mem_ok = ParseMem::with_cap(TRANSFORM_MATRIX_PRECHARGE);
        mem_ok
            .charge(TRANSFORM_MATRIX_PRECHARGE, "metashape_frames")
            .expect("4096-cap funds TRANSFORM_MATRIX_PRECHARGE");
    }

    #[test]
    fn transform_matrix_precharge_rejects_before_json_alloc() {
        // Cap below value precharge: charge must fail before json! would run.
        let need = TRANSFORM_MATRIX_PRECHARGE;
        let mut mem = ParseMem::with_cap(need.saturating_sub(1));
        let e = mem
            .charge(TRANSFORM_MATRIX_PRECHARGE, "metashape_frames")
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
        assert_eq!(
            e.context_value(spirula_core::ContextKey::Field),
            Some("metashape_frames")
        );
    }

    #[test]
    fn transform_matrix_map_insert_precharge_before_mutation() {
        let mut frame = json!({"camera_model":"PINHOLE","w":64,"h":48});
        let tm = json!([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let key = "transform_matrix";
        let insert = transform_matrix_map_insert_precharge(key);
        assert!(insert > 0);
        // Budget exactly one byte short of insert charge → reject before mutate.
        let mut mem = ParseMem::with_cap(insert.saturating_sub(1));
        let e = mem
            .charge(
                transform_matrix_map_insert_precharge(key),
                "metashape_frames",
            )
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
        assert!(frame.get(key).is_none(), "must not mutate after reject");
        // With enough budget, charge then assign succeeds.
        let mut mem = ParseMem::with_cap(insert);
        mem.charge(
            transform_matrix_map_insert_precharge(key),
            "metashape_frames",
        )
        .unwrap();
        frame[key] = tm;
        assert!(frame.get(key).is_some());
    }
}
