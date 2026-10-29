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
};
use crate::nerfstudio::load_nerfstudio_meta_value;

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

fn split_tokens(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_owned()).collect()
}

fn norm_slashes(s: &str) -> String {
    s.replace('\\', "/")
}

fn match_substring(names: &[String], s: &str) -> Vec<String> {
    let q = norm_slashes(s);
    for len in (1..=q.len()).rev() {
        let needle = &q[..len];
        let matches: Vec<String> = names
            .iter()
            .filter(|n| n.contains(needle))
            .cloned()
            .collect();
        if !matches.is_empty() {
            return matches;
        }
    }
    Vec::new()
}

fn list_image_filenames(root: &Path, image_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(walk) = std::fs::read_dir(image_dir) else {
        return out;
    };
    // Non-recursive first-level + one level of subdirs is enough for fixtures;
    // mirror C++ recursive_directory_iterator for correctness on nested trees.
    fn walk_dir(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                walk_dir(&p, root, out);
            } else if p.is_file()
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    walk_dir(image_dir, root, &mut out);
    let _ = walk;
    out
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
    let doc = parse_xml_bytes(&bytes)?;
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
                component_transforms.insert(cid.clone(), m);
            }
            let mut ids = Vec::new();
            for ci in component.iter("camera_ids") {
                ids.extend(split_tokens(&ci.text));
            }
            if !ids.is_empty() {
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
        list_image_filenames(dataset_dir, &image_dir)
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

        let mut frame = sensor.clone();
        frame["file_path"] = json!(matches[0]);
        valid_order.push(cam_id.to_owned());
        valid_cameras.insert(
            cam_id.to_owned(),
            ValidCamera {
                transform: m,
                component_id: camera.attr("component_id").unwrap_or("").to_owned(),
                frame,
            },
        );
    }

    let mut group: Vec<String> = Vec::new();
    if component_groups.is_empty() {
        group = valid_order.clone();
    } else {
        let mut best = 0usize;
        let mut best_count = 0usize;
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
        let mut ids = component_groups[best].1.clone();
        ids.sort();
        for id in &valid_order {
            if ids.binary_search(id).is_ok() {
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
        let tm = json!([
            [m[0], m[1], m[2], m[3]],
            [m[4], m[5], m[6], m[7]],
            [m[8], m[9], m[10], m[11]],
            [m[12], m[13], m[14], m[15]],
        ]);
        vc.frame["transform_matrix"] = tm;
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

    let mut meta = json!({ "frames": frames });
    if let Some(ply) = ply_path {
        let rel = ply
            .strip_prefix(dataset_dir)
            .unwrap_or(ply)
            .to_string_lossy()
            .replace('\\', "/");
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
    let meta = metashape_meta_from_xml(dataset_dir, &xml_path, ply_ref, cfg, cancel, &mut mem)?;
    cp.poll_now()?;

    let mut loaded = load_nerfstudio_meta_value(
        &meta,
        dataset_dir,
        cfg,
        cancel,
        memory_budget_bytes.saturating_sub(mem.used()),
    )?;
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
