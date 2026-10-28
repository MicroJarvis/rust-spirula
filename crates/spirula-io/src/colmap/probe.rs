//! Reconstruction directory probing and format auto-detection.

use std::fs;
use std::path::{Path, PathBuf};

use spirula_core::ErrorCode;

use super::budget::{err, err_path};
use super::types::{ColmapFileFmt, ColmapModelFmt, DatasetFormat, DatasetProbe};

fn file_fmt(dir: &Path, base: &str) -> ColmapFileFmt {
    if dir.join(format!("{base}.bin")).is_file() {
        ColmapFileFmt::Bin
    } else if dir.join(format!("{base}.txt")).is_file() {
        ColmapFileFmt::Text
    } else {
        ColmapFileFmt::None
    }
}

pub fn colmap_model_fmt(dir: &Path) -> ColmapModelFmt {
    ColmapModelFmt {
        cameras: file_fmt(dir, "cameras"),
        images: file_fmt(dir, "images"),
        points3d: file_fmt(dir, "points3D"),
    }
}

/// Cheap image-count probe for ranking COLMAP models (`colmap_model_num_images`).
pub fn colmap_model_num_images(dir: &Path) -> i64 {
    let bin = dir.join("images.bin");
    if bin.is_file() {
        if let Ok(bytes) = fs::read(&bin)
            && bytes.len() >= 8
        {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[..8]);
            return u64::from_le_bytes(b) as i64;
        }
        return -1;
    }
    let txt = dir.join("images.txt");
    if !txt.is_file() {
        return -1;
    }
    let Ok(text) = fs::read_to_string(&txt) else {
        return -1;
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('#')
            && let Some(idx) = rest.find("Number of images:")
        {
            let after = &rest[idx + "Number of images:".len()..];
            if let Ok(n) = after.trim().parse::<i64>() {
                return n;
            }
        }
    }
    let rows = text
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .count();
    (rows as i64) / 2
}

fn looks_like_metashape_xml(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let n = bytes.len().min(4096);
    let head = String::from_utf8_lossy(&bytes[..n]).to_ascii_lowercase();
    head.contains("metashape") || head.contains("photoscan") || head.contains("<chunk")
}

fn metashape_here(dataset_dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dataset_dir) else {
        return false;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if ext == "psx" {
            return true;
        }
        if ext == "xml" && looks_like_metashape_xml(&p) {
            return true;
        }
    }
    false
}

/// Locate a COLMAP reconstruction under `dataset_dir` (`find_colmap_recon`).
pub fn find_colmap_recon(
    dataset_dir: &Path,
    recon_dir: Option<&str>,
) -> Result<(PathBuf, ColmapModelFmt, Option<String>), spirula_core::DomainError> {
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

    let mut probe: Vec<String> = Vec::new();
    if let Some(r) = recon_dir.filter(|s| !s.is_empty()) {
        probe.push(r.to_owned());
    } else {
        let mut models: Vec<(i64, String)> = Vec::new();
        for parent in ["sparse", "colmap/sparse"] {
            let parent_path = dataset_dir.join(parent);
            if !parent_path.is_dir() {
                continue;
            }
            if let Ok(rd) = fs::read_dir(&parent_path) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    if !p.is_dir() {
                        continue;
                    }
                    let n = colmap_model_num_images(&p);
                    if n >= 0 {
                        let name = ent.file_name().to_string_lossy().into_owned();
                        models.push((n, format!("{parent}/{name}")));
                    }
                }
            }
        }
        models.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, rel) in models {
            probe.push(rel);
        }
        for fallback in ["sparse/0", "colmap/sparse/0", "sparse", "colmap", ""] {
            if !probe.iter().any(|p| p == fallback) {
                probe.push(fallback.to_owned());
            }
        }
    }

    let mut near_miss: Option<String> = None;
    for rel in &probe {
        let d = if rel.is_empty() {
            dataset_dir.to_path_buf()
        } else {
            dataset_dir.join(rel)
        };
        let fmt = colmap_model_fmt(&d);
        if fmt.has_recon() {
            return Ok((d, fmt, None));
        }
        let any = fmt.cameras != ColmapFileFmt::None
            || fmt.images != ColmapFileFmt::None
            || fmt.points3d != ColmapFileFmt::None;
        if any && near_miss.is_none() {
            let mut missing = Vec::new();
            if fmt.cameras == ColmapFileFmt::None {
                missing.push("cameras");
            }
            if fmt.images == ColmapFileFmt::None {
                missing.push("images");
            }
            near_miss = Some(format!(
                "{} (missing {})",
                if rel.is_empty() { "." } else { rel },
                missing.join(", ")
            ));
        }
    }
    Err(err(
        ErrorCode::NotFound,
        "recon_dir",
        match &near_miss {
            Some(m) => format!("no COLMAP reconstruction found; closest match: {m}"),
            None => "no COLMAP reconstruction found".into(),
        },
    ))
}

/// Probe a dataset directory: COLMAP available, or typed unavailable for NS/Metashape.
pub fn probe_dataset(
    dataset_dir: &Path,
    recon_dir: Option<&str>,
) -> Result<DatasetProbe, spirula_core::DomainError> {
    if !dataset_dir.exists() {
        return Err(err_path(
            ErrorCode::NotFound,
            &dataset_dir.display().to_string(),
            "dataset path does not exist",
        ));
    }
    if !dataset_dir.is_dir() {
        return Err(err_path(
            ErrorCode::InvalidArgument,
            &dataset_dir.display().to_string(),
            "dataset path is not a directory",
        ));
    }

    // Nerfstudio wins when transforms.json is present (pin auto-detect order).
    if dataset_dir.join("transforms.json").is_file() {
        return Ok(DatasetProbe::Unavailable {
            format: DatasetFormat::Nerfstudio,
            reason: "Nerfstudio transforms.json detected; parser lands in P3-F04".into(),
        });
    }

    match find_colmap_recon(dataset_dir, recon_dir) {
        Ok((recon, _, _)) => Ok(DatasetProbe::Available {
            format: DatasetFormat::Colmap,
            recon_dir: recon,
        }),
        Err(e) => {
            if metashape_here(dataset_dir) {
                Ok(DatasetProbe::Unavailable {
                    format: DatasetFormat::Metashape,
                    reason: "Metashape markers detected; parser lands in P3-F04".into(),
                })
            } else {
                // Preserve near-miss detail when nothing else matches.
                Ok(DatasetProbe::NotADataset {
                    hint: e.to_string(),
                })
            }
        }
    }
}

/// Prefer `.bin` path when both exist; else `.txt`; else `None`.
pub fn preferred_table_path(recon_dir: &Path, base: &str, fmt: ColmapFileFmt) -> Option<PathBuf> {
    match fmt {
        ColmapFileFmt::Bin => Some(recon_dir.join(format!("{base}.bin"))),
        ColmapFileFmt::Text => Some(recon_dir.join(format!("{base}.txt"))),
        ColmapFileFmt::None => None,
    }
}
