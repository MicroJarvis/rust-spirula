//! Reconstruction directory probing and format auto-detection.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use spirula_core::{CancelToken, ErrorCode};

use super::budget::{ParseCancel, err, err_path};
use super::types::{ColmapFileFmt, ColmapModelFmt, DatasetFormat, DatasetProbe};

/// Metashape header scan window (pin `ColmapParser.cpp:977-1005`).
const METASHAPE_SCAN_BYTES: usize = 8192;

/// Line-buffered stream chunk for cancel checkpoints while counting images.txt.
const IMAGE_COUNT_CANCEL_LINES: usize = 4096;

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
///
/// When `images.bin` exists it is preferred; a truncated/bad bin returns `-1`
/// and does **not** fall back to `images.txt` (pin per-file bin preference).
/// Text path streams the whole file (memory-bounded `BufRead`) unless a
/// standard `# Number of images:` header early-returns via atoll.
pub fn colmap_model_num_images(dir: &Path) -> i64 {
    colmap_model_num_images_cancelable(dir, None).unwrap_or(-1)
}

/// Same as [`colmap_model_num_images`] with cancel checkpoints on the text stream.
pub fn colmap_model_num_images_cancelable(
    dir: &Path,
    cancel: Option<&CancelToken>,
) -> Result<i64, spirula_core::DomainError> {
    let bin = dir.join("images.bin");
    if bin.is_file() {
        let Some(bytes) = read_prefix(&bin, 8) else {
            return Ok(-1);
        };
        if bytes.len() < 8 {
            return Ok(-1);
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[..8]);
        return Ok(u64::from_le_bytes(b) as i64);
    }
    let txt = dir.join("images.txt");
    if !txt.is_file() {
        return Ok(-1);
    }
    let f = File::open(&txt).map_err(|e| {
        err_path(
            ErrorCode::ReadFailed,
            &txt.display().to_string(),
            format!("cannot open images.txt: {e}"),
        )
    })?;
    // Small reusable buffer — never load the whole table for ranking.
    let mut reader = BufReader::with_capacity(64 * 1024, f);
    let mut line = String::new();
    let mut rows: i64 = 0;
    let mut cp = ParseCancel::new(cancel);
    let mut lines_seen = 0usize;
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &txt.display().to_string(),
                format!("cannot read images.txt: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        lines_seen += 1;
        if lines_seen.is_multiple_of(IMAGE_COUNT_CANCEL_LINES) {
            cp.poll_now()?;
        }
        // Strip trailing newline(s) without allocating.
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        if line.as_bytes()[0] == b'#' {
            if let Some(idx) = line.find("Number of images:") {
                let after = &line[idx + "Number of images:".len()..];
                if let Some(n) = parse_leading_i64(after) {
                    return Ok(n);
                }
            }
            continue;
        }
        rows += 1;
        cp.tick(1)?;
    }
    Ok(rows / 2)
}

/// Within 8192 bytes require `<document` and (`psx:` + `path=` for `.psx`, or
/// `<chunk` for `.xml`). Bare `meta` / `<chunk/>` must not match.
fn looks_like_metashape(path: &Path, is_psx: bool) -> bool {
    let Some(bytes) = read_prefix(path, METASHAPE_SCAN_BYTES) else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes);
    if !head.contains("<document") {
        return false;
    }
    if is_psx {
        head.contains("psx:") && head.contains("path=")
    } else {
        head.contains("<chunk")
    }
}

fn metashape_here(dataset_dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dataset_dir) else {
        return false;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if !p.is_file() {
            continue;
        }
        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if ext == "psx" && looks_like_metashape(&p, true) {
            return true;
        }
        if ext == "xml" && looks_like_metashape(&p, false) {
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
                    let n = colmap_model_num_images_cancelable(&p, cancel)?;
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
            if e.code() == ErrorCode::Canceled {
                return Err(e);
            }
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
    fn metashape_requires_document_and_second_marker() {
        let h = TempHarness::new().unwrap();
        let bad_psx = h.path().join("bad.psx");
        std::fs::write(&bad_psx, b"meta").unwrap();
        assert!(!looks_like_metashape(&bad_psx, true));

        let bad_xml = h.path().join("bad.xml");
        std::fs::write(&bad_xml, b"<chunk/>").unwrap();
        assert!(!looks_like_metashape(&bad_xml, false));

        let path_only = h.path().join("path_only.psx");
        std::fs::write(&path_only, b"<document path=\"x\"/>").unwrap();
        assert!(
            !looks_like_metashape(&path_only, true),
            ".psx requires psx: namespace marker"
        );

        let good_psx = h.path().join("good.psx");
        std::fs::write(&good_psx, b"<document psx:version=\"1\" path=\"x\"/>").unwrap();
        assert!(looks_like_metashape(&good_psx, true));

        let good_xml = h.path().join("good.xml");
        std::fs::write(&good_xml, b"<document><chunk/></document>").unwrap();
        assert!(!looks_like_metashape(&good_xml, true));
        assert!(looks_like_metashape(&good_xml, false));

        // Marker after 8192 is missed.
        let late = h.path().join("late.xml");
        let mut f = File::create(&late).unwrap();
        f.write_all(&vec![b'x'; METASHAPE_SCAN_BYTES]).unwrap();
        f.write_all(b"<document><chunk/></document>").unwrap();
        assert!(!looks_like_metashape(&late, false));
    }

    #[test]
    fn metashape_probe_rejects_unrelated_markers() {
        let h = TempHarness::new().unwrap();
        let ms = h.path().join("ms");
        std::fs::create_dir_all(&ms).unwrap();
        std::fs::write(ms.join("project.psx"), "meta").unwrap();
        match probe_dataset(&ms, None).unwrap() {
            DatasetProbe::NotADataset { .. } => {}
            other => panic!("expected not-a-dataset for bad.psx, got {other:?}"),
        }
        std::fs::remove_file(ms.join("project.psx")).unwrap();
        std::fs::write(ms.join("export.xml"), "<chunk/>").unwrap();
        match probe_dataset(&ms, None).unwrap() {
            DatasetProbe::NotADataset { .. } => {}
            other => panic!("expected not-a-dataset for bare <chunk/>, got {other:?}"),
        }
        std::fs::write(ms.join("export.xml"), "<document><chunk/></document>").unwrap();
        match probe_dataset(&ms, None).unwrap() {
            DatasetProbe::Unavailable {
                format: DatasetFormat::Metashape,
                ..
            } => {}
            other => panic!("expected metashape unavailable, got {other:?}"),
        }
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
    fn images_txt_full_stream_count_without_header() {
        let h = TempHarness::new().unwrap();
        let dir = h.path().join("m");
        std::fs::create_dir_all(&dir).unwrap();
        // 1000 images × (header + empty POINTS2D) with long names — beyond any
        // former 64 KiB approximate prefix.
        let mut body = String::new();
        for i in 0..1000 {
            // Non-empty POINTS2D so both lines count (empty lines are skipped).
            body.push_str(&format!(
                "{i} 1 0 0 0 0 0 0 1 {}.png\n0 0 -1\n",
                "a".repeat(100)
            ));
        }
        std::fs::write(dir.join("images.txt"), body).unwrap();
        assert_eq!(colmap_model_num_images(&dir), 1000);
    }

    #[test]
    fn recon_prefers_full_stream_count_over_headered_smaller() {
        let h = TempHarness::new().unwrap();
        let ds = h.path().join("ds");
        let large = ds.join("sparse/large");
        let small = ds.join("sparse/small");
        for p in [&large, &small] {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(p.join("cameras.txt"), "1 PINHOLE 64 48 100 100 32 24\n").unwrap();
        }
        let mut rows = String::new();
        for i in 0..1000 {
            rows.push_str(&format!(
                "{i} 1 0 0 0 0 0 0 1 {}.png\n0 0 -1\n",
                "a".repeat(100)
            ));
        }
        std::fs::write(large.join("images.txt"), rows).unwrap();
        std::fs::write(
            small.join("images.txt"),
            "# Number of images: 700, mean observations per image: 0\n",
        )
        .unwrap();
        assert_eq!(colmap_model_num_images(&large), 1000);
        assert_eq!(colmap_model_num_images(&small), 700);
        let (chosen, _, _) = find_colmap_recon(&ds, None).unwrap();
        assert!(
            chosen.ends_with("sparse/large"),
            "expected sparse/large, got {}",
            chosen.display()
        );
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
        write_recon(&ds, "sparse/b", 10);
        write_recon(&ds, "sparse/a", 10);
        write_recon(&ds, "sparse/z", 3);
        let (chosen, _, _) = find_colmap_recon(&ds, None).unwrap();
        assert!(
            chosen.ends_with("sparse/a"),
            "expected sparse/a (count desc, path asc), got {}",
            chosen.display()
        );

        write_recon(&ds, "sparse/c", 50);
        let (chosen, _, _) = find_colmap_recon(&ds, None).unwrap();
        assert!(
            chosen.ends_with("sparse/c"),
            "expected sparse/c (count 50), got {}",
            chosen.display()
        );
    }
}
