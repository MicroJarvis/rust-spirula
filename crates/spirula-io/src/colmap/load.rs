//! Production COLMAP load pipeline (P3-F03). No scene center/normalize (P3-F05).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use spirula_core::{CancelToken, ErrorCode};

use super::bake::bake_and_fit_cancelable;
use super::binary::{read_cameras_binary, read_images_binary, read_points3d_binary};
use super::budget::{
    MAX_LOAD_MEMORY_BYTES, ParseCancel, ParseMem, err, err_path, try_reserve_elems,
};
use super::config::ColmapLoadConfig;
use super::pose::colmap_to_c2w;
use super::probe::{
    colmap_model_fmt, find_colmap_recon_cancelable, preferred_table_path, probe_dataset_cancelable,
};
use super::split::compute_splits;
use super::text::{read_cameras_text, read_images_text, read_points3d_text};
use super::types::DatasetProbe;
use super::types::{
    BakedIntrinsics, ColmapCamera, ColmapFileFmt, ColmapImage, ColmapObservation, ColmapPoint3D,
    ColmapPoints3D, DatasetFormat, DatasetLoadSummary, LoadedFrame, ParsedDataset, SplitIndices,
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

/// Charge live camera table into the load-wide cumulative budget.
fn charge_cameras(
    mem: &mut ParseMem,
    cameras: &BTreeMap<i32, ColmapCamera>,
) -> Result<(), spirula_core::DomainError> {
    mem.charge_btree_entries::<(i32, ColmapCamera)>(cameras.len(), "cameras")?;
    for cam in cameras.values() {
        mem.charge_str_bytes(cam.model.len(), "cameras")?;
        mem.charge_elems::<f64>(cam.params.len(), "cameras")?;
    }
    Ok(())
}

/// Charge live image table (including observation payloads).
fn charge_images(
    mem: &mut ParseMem,
    images: &BTreeMap<i32, ColmapImage>,
) -> Result<(), spirula_core::DomainError> {
    mem.charge_btree_entries::<(i32, ColmapImage)>(images.len(), "images")?;
    for im in images.values() {
        mem.charge_str_bytes(im.name.len(), "images")?;
        mem.charge_elems::<ColmapObservation>(im.observations.len(), "observations")?;
    }
    Ok(())
}

/// Charge live points + tracks.
fn charge_points(
    mem: &mut ParseMem,
    points: &ColmapPoints3D,
) -> Result<(), spirula_core::DomainError> {
    mem.charge_elems::<ColmapPoint3D>(points.points.len(), "points3D")?;
    for pt in &points.points {
        mem.charge_elems::<super::types::ColmapTrackElem>(pt.track.len(), "track")?;
    }
    Ok(())
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
    load_colmap_dataset_budgeted(dataset_dir, cfg, cancel, MAX_LOAD_MEMORY_BYTES)
}

/// Same as [`load_colmap_dataset`] with an explicit cumulative live-structure
/// budget (bytes). Used by tests to inject small-budget cross-table / emit OOM.
pub fn load_colmap_dataset_budgeted(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
    memory_budget_bytes: u64,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    let mut mem = ParseMem::with_cap(memory_budget_bytes);
    cp.poll_now()?;

    // Auto-detect / typed unavailable for other formats.
    match probe_dataset_cancelable(dataset_dir, cfg.recon_dir.as_deref(), cancel)? {
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
    let (recon_dir, _, _) = find_colmap_recon_cancelable(dataset_dir, recon_hint, cancel)?;
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
    charge_cameras(&mut mem, &cameras)?;
    cp.poll_now()?;
    let images = read_images(&recon_dir, fmt.images, cancel)?;
    charge_images(&mut mem, &images)?;
    cp.poll_now()?;
    let points = read_points(&recon_dir, fmt.points3d, cancel)?;
    charge_points(&mut mem, &points)?;
    cp.poll_now()?;

    // Bake once per camera id. Cancelable fit polls azimuth/candidate steps;
    // dense mid-`fit_model` LM remains non-preemptible. Poll around each cam.
    mem.charge_btree_entries::<(i32, BakedIntrinsics)>(cameras.len(), "baked")?;
    let mut baked = BTreeMap::new();
    for (id, cam) in &cameras {
        cp.poll_now()?;
        baked.insert(*id, bake_and_fit_cancelable(cam, cancel)?);
        cp.poll_now()?;
    }

    // Association set for track checks (before consuming `images`).
    mem.charge_btree_entries::<i32>(images.len(), "image_ids")?;
    let image_ids: BTreeSet<i32> = images.keys().copied().collect();

    // Verify point tracks reference known image ids when present.
    for (pi, pt) in points.points.iter().enumerate() {
        cp.tick(1)?;
        for (ti, t) in pt.track.iter().enumerate() {
            cp.tick(1)?;
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

    // Consume images into builds (move observations — no full clone).
    let image_dir = dataset_dir.join(&cfg.image_dir);
    let mut missing = Vec::new();
    let n_images = images.len();
    let mut builds = Vec::new();
    try_reserve_elems(&mut builds, n_images, &mut mem, "builds")?;
    for (_, im) in images {
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
        mem.charge_str_bytes(sort_key.len(), "sort_key")?;
        mem.charge_str_bytes(path.as_os_str().len(), "path")?;
        builds.push(FrameBuild { im, path, sort_key });
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

    let mut names = Vec::new();
    try_reserve_elems(&mut names, builds.len(), &mut mem, "names")?;
    for b in &builds {
        cp.tick(1)?;
        let s = b.path.display().to_string();
        mem.charge_str_bytes(s.len(), "names")?;
        names.push(s);
    }
    let split_cfg = cfg.split_config();
    // Indices in the emitted-frame domain; validation applied once inside.
    let plan = compute_splits(&names, &split_cfg)?;
    mem.charge_elems::<usize>(plan.emit.len(), "split_emit")?;
    mem.charge_elems::<usize>(plan.train.len(), "split_train")?;
    mem.charge_elems::<usize>(plan.eval.len(), "split_eval")?;
    mem.charge_elems::<usize>(plan.validation.len(), "split_validation")?;

    // Empty eval split is legal (caller skips eval); empty train already erred.
    // Move observations / name / path out of builds for emitted frames only.
    let mut frames = Vec::new();
    try_reserve_elems(&mut frames, plan.emit.len(), &mut mem, "frames")?;
    for &i in &plan.emit {
        cp.tick(1)?;
        let b = &mut builds[i];
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
        let name = std::mem::take(&mut b.im.name);
        let path = std::mem::take(&mut b.path);
        let observations = std::mem::take(&mut b.im.observations);
        frames.push(LoadedFrame {
            image_id: b.im.image_id,
            camera_id: b.im.camera_id,
            name,
            path,
            c2w,
            width,
            height,
            intrins: [bi.fx, bi.fy, bi.cx, bi.cy],
            model: bi.model,
            distortion: bi.distortion,
            dist_coeffs: bi.dist,
            redistort: bi.source.clone(),
            observations,
        });
    }

    let split = SplitIndices {
        train: plan.train,
        eval: plan.eval,
        validation: plan.validation,
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
    let _ = mem;
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
    use super::*;
    use spirula_core::ErrorCode;
    use std::fs;

    use crate::temp_harness::TempHarness;

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

    fn write_min_recon(dir: &Path, n_cams: usize, n_images: usize, model: &str, params: &str) {
        fs::create_dir_all(dir).unwrap();
        let mut cams = String::new();
        for i in 1..=n_cams {
            cams.push_str(&format!("{i} {model} 64 48 {params}\n"));
        }
        fs::write(dir.join("cameras.txt"), cams).unwrap();
        let mut imgs = String::new();
        for i in 1..=n_images {
            let cam = ((i - 1) % n_cams) + 1;
            imgs.push_str(&format!("{i} 1 0 0 0 0 0 0 {cam} frame_{i:04}.png\n\n"));
        }
        fs::write(dir.join("images.txt"), imgs).unwrap();
    }

    #[test]
    fn small_budget_rejects_cross_table_accumulation() {
        let h = TempHarness::new().unwrap();
        let ds = h.path().join("ds");
        write_min_recon(&ds, 4, 1, "PINHOLE", "100 100 32 24");
        // Enough for cameras charge, too small once images join the live set.
        let err = load_colmap_dataset_budgeted(
            &ds,
            &ColmapLoadConfig {
                require_image_files: false,
                ..Default::default()
            },
            None,
            256,
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn small_budget_rejects_emit_phase() {
        let h = TempHarness::new().unwrap();
        let ds = h.path().join("ds");
        // Many frames so names/frames/split vectors dominate after tables fit.
        write_min_recon(&ds, 1, 32, "PINHOLE", "100 100 32 24");
        let ok = load_colmap_dataset(
            &ds,
            &ColmapLoadConfig {
                require_image_files: false,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(ok.frames.len(), 32);
        assert_eq!(ok.split.train.len(), 32);

        // Cap sits above table+build charges but below names/frames/splits.
        let err = load_colmap_dataset_budgeted(
            &ds,
            &ColmapLoadConfig {
                require_image_files: false,
                ..Default::default()
            },
            None,
            12_000,
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn default_budget_loads_minimal_recon() {
        let h = TempHarness::new().unwrap();
        let ds = h.path().join("ds");
        write_min_recon(&ds, 1, 1, "PINHOLE", "100 100 32 24");
        let loaded = load_colmap_dataset(
            &ds,
            &ColmapLoadConfig {
                require_image_files: false,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(loaded.frames.len(), 1);
        assert_eq!(loaded.split.train, vec![0]);
    }
}
