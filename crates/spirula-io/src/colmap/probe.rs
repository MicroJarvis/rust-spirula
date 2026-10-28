//! Reconstruction directory probing and format auto-detection.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use spirula_core::{CancelToken, ErrorCode};

use super::budget::{ParseCancel, err, err_path};
use super::types::{ColmapFileFmt, ColmapModelFmt, DatasetFormat, DatasetProbe};

/// Metashape XML header scan window (dual markers: metashape / photoscan / `<chunk`).
const METASHAPE_SCAN_BYTES: usize = 8192;

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

fn read_prefix(path: &Path, max: usize) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

/// `atoll`-style leading integer (pin `ColmapParser.cpp:600`). Accepts a
/// leading count before trailing commentary such as
/// `# Number of images: 10, mean observations per image: 0`.
fn parse_leading_i64(s: &str) -> Option<i64> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0usize;
    if bytes[0] == b'+' || bytes[0] == b'-' {
        i = 1;
    }
    let start_digits = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == start_digits {
        return None;
    }
    s[..i].parse().ok()
}

/// Cheap image-count probe for ranking COLMAP models (`colmap_model_num_images`).
/// Reads only a bounded header / prefix — never the whole file.
///
/// When `images.bin` exists it is preferred; a truncated/bad bin returns `-1`
/// and does **not** fall back to `images.txt` (pin per-file bin preference).
pub fn colmap_model_num_images(dir: &Path) -> i64 {
    let bin = dir.join("images.bin");
    if bin.is_file() {
        let Some(bytes) = read_prefix(&bin, 8) else {
            return -1;
        };
        if bytes.len() < 8 {
            return -1;
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[..8]);
        return u64::from_le_bytes(b) as i64;
    }
    let txt = dir.join("images.txt");
    if !txt.is_file() {
        return -1;
    }
    // Scan a bounded prefix for the `# Number of images:` comment, else count
    // non-comment rows in that prefix (approximate; ranking only).
    // 64 KiB covers typical COLMAP headers without scanning the whole table.
    const IMAGES_TXT_PREFIX: usize = 64 * 1024;
    let Some(bytes) = read_prefix(&txt, IMAGES_TXT_PREFIX) else {
        return -1;
    };
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('#')
            && let Some(idx) = rest.find("Number of images:")
        {
            let after = &rest[idx + "Number of images:".len()..];
            if let Some(n) = parse_leading_i64(after) {
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
    let Some(bytes) = read_prefix(path, METASHAPE_SCAN_BYTES) else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    head.contains("metashape") || head.contains("photoscan") || head.contains("<chunk")
}

fn metashape_here(dataset_dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dataset_dir) else {
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
    find_colmap_recon_cancelable(dataset_dir, recon_dir, None)
}

/// Same as [`find_colmap_recon`] with cancel checkpoints on dir scan/sort.
pub fn find_colmap_recon_cancelable(
    dataset_dir: &Path,
    recon_dir: Option<&str>,
    cancel: Option<&CancelToken>,
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

    let mut cp = ParseCancel::new(cancel);
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
            if let Ok(rd) = std::fs::read_dir(&parent_path) {
                for ent in rd.flatten() {
                    cp.tick(1)?;
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
        cp.poll_now()?;
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
        cp.tick(1)?;
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
    probe_dataset_cancelable(dataset_dir, recon_dir, None)
}

/// Same as [`probe_dataset`] with cancel checkpoints.
pub fn probe_dataset_cancelable(
    dataset_dir: &Path,
    recon_dir: Option<&str>,
    cancel: Option<&CancelToken>,
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

    match find_colmap_recon_cancelable(dataset_dir, recon_dir, cancel) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_harness::TempHarness;
    use std::io::Write;

    #[test]
    fn metashape_scan_is_bounded_8192() {
        let h = TempHarness::new().unwrap();
        let p = h.path().join("big.xml");
        let mut f = File::create(&p).unwrap();
        // Marker after 8192 would be missed — pin dual-marker window.
        let pad = vec![b'x'; METASHAPE_SCAN_BYTES];
        f.write_all(&pad).unwrap();
        f.write_all(b"<chunk>metashape</chunk>").unwrap();
        assert!(!looks_like_metashape_xml(&p));
        let mut f = File::create(&p).unwrap();
        f.write_all(b"<chunk>metashape</chunk>").unwrap();
        assert!(looks_like_metashape_xml(&p));
    }

    #[test]
    fn images_bin_probe_reads_eight_bytes_only() {
        let h = TempHarness::new().unwrap();
        let dir = h.path().join("m");
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = 7u64.to_le_bytes().to_vec();
        bytes.extend(std::iter::repeat_n(0u8, 64 * 1024));
        std::fs::write(dir.join("images.bin"), &bytes).unwrap();
        assert_eq!(colmap_model_num_images(&dir), 7);
    }

    #[test]
    fn comma_header_parses_leading_count_atoll_style() {
        // Probe counterexample vs tip 6bca1fc: whole-string parse returned 0.
        let h = TempHarness::new().unwrap();
        let dir = h.path().join("m");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("images.txt"),
            "# Number of images: 10, mean observations per image: 0\n",
        )
        .unwrap();
        assert_eq!(colmap_model_num_images(&dir), 10);
    }

    #[test]
    fn bad_bin_does_not_fallback_to_txt_count() {
        let h = TempHarness::new().unwrap();
        let dir = h.path().join("m");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("images.bin"), [1u8, 2, 3]).unwrap(); // < 8 bytes
        std::fs::write(
            dir.join("images.txt"),
            "# Number of images: 99, mean observations per image: 0\n",
        )
        .unwrap();
        assert_eq!(colmap_model_num_images(&dir), -1);
        // Preferred table path still picks bin when present.
        let fmt = colmap_model_fmt(&dir);
        assert_eq!(fmt.images, ColmapFileFmt::Bin);
        assert_eq!(
            preferred_table_path(&dir, "images", fmt.images),
            Some(dir.join("images.bin"))
        );
    }

    fn write_recon(root: &Path, rel: &str, count_header: i64) {
        let d = root.join(rel);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("cameras.txt"), "1 PINHOLE 64 48 100 100 32 24\n").unwrap();
        std::fs::write(
            d.join("images.txt"),
            format!("# Number of images: {count_header}, mean observations per image: 0\n"),
        )
        .unwrap();
    }

    #[test]
    fn recon_sort_count_desc_then_path_asc() {
        let h = TempHarness::new().unwrap();
        let ds = h.path().join("ds");
        std::fs::create_dir_all(ds.join("images")).unwrap();
        // Equal high count: sparse/a vs sparse/b → path ascending picks `a`.
        write_recon(&ds, "sparse/b", 10);
        write_recon(&ds, "sparse/a", 10);
        // Lower count must lose to 10.
        write_recon(&ds, "sparse/z", 3);
        let (chosen, _, _) = find_colmap_recon(&ds, None).unwrap();
        assert!(
            chosen.ends_with("sparse/a"),
            "expected sparse/a (count desc, path asc), got {}",
            chosen.display()
        );

        // Higher count wins over path order.
        write_recon(&ds, "sparse/c", 50);
        let (chosen, _, _) = find_colmap_recon(&ds, None).unwrap();
        assert!(
            chosen.ends_with("sparse/c"),
            "expected sparse/c (count 50), got {}",
            chosen.display()
        );
    }
}
