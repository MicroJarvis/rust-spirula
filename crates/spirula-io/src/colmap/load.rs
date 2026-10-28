//! Production COLMAP load pipeline (P3-F03). No scene center/normalize (P3-F05).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use spirula_core::{CancelToken, ErrorCode};

use super::bake::{bake_and_fit, validate_equirectangular};
use super::binary::{read_cameras_binary, read_images_binary, read_points3d_binary};
use super::budget::{ParseCancel, err, err_path};
use super::config::ColmapLoadConfig;
use super::pose::colmap_to_c2w;
use super::probe::{colmap_model_fmt, find_colmap_recon, preferred_table_path, probe_dataset};
use super::split::compute_splits;
use super::text::{read_cameras_text, read_images_text, read_points3d_text};
use super::types::DatasetProbe;
use super::types::{
    ColmapCamera, ColmapFileFmt, ColmapImage, ColmapPoints3D, DatasetFormat, DatasetLoadSummary,
    LoadedFrame, ParsedDataset, SplitIndices,
};

fn read_cameras(
    recon: &Path,
    fmt: ColmapFileFmt,
    cancel: Option<&CancelToken>,
) -> Result<BTreeMap<i32, ColmapCamera>, spirula_core::DomainError> {
    let path = preferred_table_path(recon, "cameras", fmt)
        .ok_or_else(|| err(ErrorCode::NotFound, "cameras", "cameras.bin/txt missing"))?;
    match fmt {
        ColmapFileFmt::Bin => read_cameras_binary(&path, cancel),
        ColmapFileFmt::Text => read_cameras_text(&path, cancel),
        ColmapFileFmt::None => Err(err(ErrorCode::NotFound, "cameras", "cameras missing")),
    }
}

fn read_images(
    recon: &Path,
    fmt: ColmapFileFmt,
    cancel: Option<&CancelToken>,
) -> Result<BTreeMap<i32, ColmapImage>, spirula_core::DomainError> {
    let path = preferred_table_path(recon, "images", fmt)
        .ok_or_else(|| err(ErrorCode::NotFound, "images", "images.bin/txt missing"))?;
    match fmt {
        ColmapFileFmt::Bin => read_images_binary(&path, cancel),
        ColmapFileFmt::Text => read_images_text(&path, cancel),
        ColmapFileFmt::None => Err(err(ErrorCode::NotFound, "images", "images missing")),
    }
}

fn read_points(
    recon: &Path,
    fmt: ColmapFileFmt,
    cancel: Option<&CancelToken>,
) -> Result<ColmapPoints3D, spirula_core::DomainError> {
    match preferred_table_path(recon, "points3D", fmt) {
        None => Ok(ColmapPoints3D::default()),
        Some(path) => match fmt {
            ColmapFileFmt::Bin => read_points3d_binary(&path, cancel),
            ColmapFileFmt::Text => read_points3d_text(&path, cancel),
            ColmapFileFmt::None => Ok(ColmapPoints3D::default()),
        },
    }
}

fn path_sort_key(path: &Path) -> String {
    // Match C++ `path.generic_string()` — forward slashes.
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

struct FrameBuild {
    im: ColmapImage,
    path: PathBuf,
    sort_key: String,
}

/// Reject names that could escape `image_dir` via `Path::join` (absolute / `..` /
/// root / prefix). Relative `Normal` and `.` components only.
pub(crate) fn validate_image_name(name: &str) -> Result<(), spirula_core::DomainError> {
    let name_path = Path::new(name);
    if name.is_empty() || name_path.as_os_str().is_empty() {
        return Err(err(
            ErrorCode::InvalidArgument,
            "name",
            "illegal empty image path",
        ));
    }
    if name_path.is_absolute() {
        return Err(err(
            ErrorCode::InvalidArgument,
            "name",
            format!("illegal absolute image path `{name}`"),
        ));
    }
    for component in name_path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(err(
                    ErrorCode::InvalidArgument,
                    "name",
                    format!("illegal image path `{name}`"),
                ));
            }
        }
    }
    Ok(())
}

/// Load a COLMAP dataset. Points optional. Nerfstudio/Metashape → typed unavailable.
pub fn load_colmap_dataset(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;

    // Auto-detect / typed unavailable for other formats.
    match probe_dataset(dataset_dir, cfg.recon_dir.as_deref())? {
        DatasetProbe::Unavailable { format, reason } => {
            return Err(err(
                ErrorCode::UnimplementedOption,
                "format",
                format!("{reason} ({})", format.as_str()),
            )
            .with_context(spirula_core::ContextKey::Format, format.as_str())
            .expect("format"));
        }
        DatasetProbe::NotADataset { hint } => {
            return Err(err(
                ErrorCode::UnsupportedFormat,
                "dataset",
                format!("not a supported dataset: {hint}"),
            ));
        }
        DatasetProbe::Available { .. } => {}
    }

    let recon_hint = cfg.recon_dir.as_deref();
    let (recon_dir, _, _) = find_colmap_recon(dataset_dir, recon_hint)?;
    // Prefer live fmt from the chosen recon (bin preferred per file).
    let fmt = colmap_model_fmt(&recon_dir);
    if !fmt.has_recon() {
        return Err(err(
            ErrorCode::NotFound,
            "recon_dir",
            "COLMAP cameras/images missing",
        ));
    }

    cp.poll_now()?;
    let cameras = read_cameras(&recon_dir, fmt.cameras, cancel)?;
    cp.poll_now()?;
    let images = read_images(&recon_dir, fmt.images, cancel)?;
    cp.poll_now()?;
    let points = read_points(&recon_dir, fmt.points3d, cancel)?;
    cp.poll_now()?;

    for cam in cameras.values() {
        validate_equirectangular(cam)?;
        for (i, v) in cam.params.iter().enumerate() {
            if !v.is_finite() {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "params",
                    format!("camera {} param {i} is nonfinite", cam.camera_id),
                ));
            }
        }
        if cam.width == 0 || cam.height == 0 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "size",
                format!(
                    "camera {} has non-positive size {}x{}",
                    cam.camera_id, cam.width, cam.height
                ),
            ));
        }
    }

    // Bake once per camera id (fit can be expensive).
    let mut baked = BTreeMap::new();
    for (id, cam) in &cameras {
        cp.tick(1)?;
        baked.insert(*id, bake_and_fit(cam)?);
    }

    let image_dir = dataset_dir.join(&cfg.image_dir);
    let mut missing = Vec::new();
    let mut builds = Vec::new();
    for im in images.values() {
        cp.tick(1)?;
        validate_image_name(&im.name)?;
        let mut path = image_dir.join(&im.name);
        if !path.is_file() {
            let alt = dataset_dir.join(&im.name);
            if alt.is_file() {
                path = alt;
            }
        }
        if cfg.require_image_files && !path.is_file() {
            missing.push(path.display().to_string());
            continue;
        }
        let sort_key = path_sort_key(&path);
        builds.push(FrameBuild {
            im: im.clone(),
            path,
            sort_key,
        });
    }
    if builds.is_empty() {
        if !missing.is_empty() {
            return Err(err_path(
                ErrorCode::NotFound,
                &missing[0],
                format!(
                    "all images missing (first: {}); check --image-dir",
                    missing[0]
                ),
            ));
        }
        return Err(err(
            ErrorCode::MalformedRecord,
            "images",
            "reconstruction has no images",
        ));
    }
    builds.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

    // Association: every kept frame must reference a known camera.
    for b in &builds {
        if !cameras.contains_key(&b.im.camera_id) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "camera_id",
                format!(
                    "image {} references missing camera id {}",
                    b.im.name, b.im.camera_id
                ),
            ));
        }
    }

    // Verify point tracks reference known image ids when present.
    let image_ids: std::collections::BTreeSet<i32> = images.keys().copied().collect();
    for (pi, pt) in points.points.iter().enumerate() {
        cp.tick(1)?;
        for (ti, t) in pt.track.iter().enumerate() {
            if !image_ids.contains(&t.image_id) {
                return Err(err(
                    ErrorCode::MalformedRecord,
                    "track",
                    format!(
                        "point {pi} track[{ti}] references missing image_id {}",
                        t.image_id
                    ),
                ));
            }
        }
        for (k, v) in pt.xyz.iter().enumerate() {
            if !v.is_finite() {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "xyz",
                    format!("point {} xyz[{k}] is nonfinite", pt.point3d_id),
                ));
            }
        }
    }

    let names: Vec<String> = builds
        .iter()
        .map(|b| b.path.display().to_string())
        .collect();
    let split_cfg = cfg.split_config();
    let (train_idx, eval_idx, val_idx) = compute_splits(&names, &split_cfg)?;

    // Which frames to emit depends on cfg.split.
    let selected = match cfg.split.as_str() {
        "eval" => eval_idx.clone(),
        "all" => (0..builds.len()).collect(),
        _ => train_idx.clone(),
    };

    // Empty eval split is legal (caller skips eval); empty train already erred.
    let mut frames = Vec::with_capacity(selected.len());
    for &i in &selected {
        cp.tick(1)?;
        let b = &builds[i];
        let cam = &cameras[&b.im.camera_id];
        let bi = &baked[&b.im.camera_id];
        let c2w = colmap_to_c2w(b.im.qvec, b.im.tvec)?;
        let width = u32::try_from(cam.width).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "width",
                format!("width {} exceeds u32", cam.width),
            )
        })?;
        let height = u32::try_from(cam.height).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "height",
                format!("height {} exceeds u32", cam.height),
            )
        })?;
        frames.push(LoadedFrame {
            image_id: b.im.image_id,
            camera_id: b.im.camera_id,
            name: b.im.name.clone(),
            path: b.path.clone(),
            c2w,
            width,
            height,
            intrins: [bi.fx, bi.fy, bi.cx, bi.cy],
            model: bi.model,
            distortion: bi.distortion,
            dist_coeffs: bi.dist,
            redistort: bi.source.clone(),
            observations: b.im.observations.clone(),
        });
    }

    // Remap split indices into the emitted frame list when split != all.
    let split = if cfg.split == "all" {
        SplitIndices {
            train: train_idx,
            eval: eval_idx,
            validation: val_idx,
        }
    } else {
        // Emitted frames are already the selected subset; train/val are local,
        // eval listing stays global over the pre-split sorted set for summaries.
        let local_n = frames.len();
        let (local_train, local_val) = super::split::assign_val_split(
            local_n as i64,
            if cfg.split == "eval" {
                0.0
            } else {
                cfg.validation_fraction
            },
        )?;
        SplitIndices {
            train: local_train,
            eval: if cfg.split == "eval" {
                (0..local_n).collect()
            } else {
                Vec::new()
            },
            validation: local_val,
        }
    };

    let summary = DatasetLoadSummary {
        path: dataset_dir.display().to_string(),
        format: DatasetFormat::Colmap.as_str().to_owned(),
        recon_dir: recon_dir.display().to_string(),
        num_cameras: cameras.len() as u64,
        num_images: frames.len() as u64,
        num_points: points.num() as u64,
        num_train: split.train.len() as u64,
        num_eval: split.eval.len() as u64,
        num_validation: split.validation.len() as u64,
    };

    cp.poll_now()?;
    Ok(ParsedDataset {
        dataset_dir: dataset_dir.to_path_buf(),
        recon_dir,
        format: DatasetFormat::Colmap,
        cameras,
        frames,
        points,
        split,
        summary,
    })
}

/// Probe then load. Same as [`load_colmap_dataset`] for COLMAP paths.
pub fn load_dataset(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    load_colmap_dataset(dataset_dir, cfg, cancel)
}

#[cfg(test)]
mod tests {
    use super::validate_image_name;
    use spirula_core::ErrorCode;

    #[test]
    fn rejects_parent_absolute_and_root_names() {
        assert!(validate_image_name("ok/rel.png").is_ok());
        assert!(validate_image_name("./ok.png").is_ok());
        assert_eq!(
            validate_image_name("../secret.png").unwrap_err().code(),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            validate_image_name("/abs/secret.png").unwrap_err().code(),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            validate_image_name("/").unwrap_err().code(),
            ErrorCode::InvalidArgument
        );
    }
}
