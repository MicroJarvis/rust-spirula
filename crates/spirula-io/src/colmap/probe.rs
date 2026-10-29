//! Reconstruction directory probing and format auto-detection.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use spirula_core::{CancelToken, ErrorCode};

use super::budget::{ParseCancel, err, err_path};
use super::types::{ColmapFileFmt, ColmapModelFmt, DatasetFormat, DatasetProbe};

/// Metashape header scan window (pin `ColmapParser.cpp:977-1005`).
const METASHAPE_SCAN_BYTES: usize = 8192;

/// Byte chunk for cancel checkpoints while streaming `images.txt` counts.
const IMAGE_COUNT_READ_CHUNK: usize = 64 * 1024;

/// Cap on comment-line prefix retained for `# Number of images:` atoll.
/// COLMAP headers are short; excess bytes are discarded until newline so a
/// single huge comment line cannot grow an unbounded `String`.
const IMAGE_COUNT_HEADER_CAP: usize = 8192;

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
/// Text path streams the whole file in byte chunks (no `read_line` into a
/// growing `String`) unless a standard `# Number of images:` header
/// early-returns via atoll.
pub fn colmap_model_num_images(dir: &Path) -> i64 {
    colmap_model_num_images_cancelable(dir, None).unwrap_or(-1)
}

/// Parse `# Number of images:` from a capped comment-line prefix.
fn header_image_count(comment_prefix: &str) -> Option<i64> {
    let idx = comment_prefix.find("Number of images:")?;
    let after = &comment_prefix[idx + "Number of images:".len()..];
    parse_leading_i64(after)
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
    count_images_txt_stream(f, &txt, cancel)
}

/// Byte-chunk `images.txt` row counter — never `read_line` into an unbounded
/// `String`. Takes any [`Read`] so after-start cancel can be proven with a
/// controlled reader (FIFO/`is_file` races are not required).
fn count_images_txt_stream(
    mut reader: impl Read,
    path_for_err: &Path,
    cancel: Option<&CancelToken>,
) -> Result<i64, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;

    let mut rows: i64 = 0;
    // Line FSM: at start → classify `#` comment vs data; `\r` ignored (CRLF).
    let mut at_line_start = true;
    let mut in_comment = false;
    let mut saw_nonempty = false;
    let mut header_prefix = String::new();
    let mut chunk = [0u8; IMAGE_COUNT_READ_CHUNK];

    loop {
        let n = reader.read(&mut chunk).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &path_for_err.display().to_string(),
                format!("cannot read images.txt: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        cp.tick(n)?;

        for &b in &chunk[..n] {
            if b == b'\r' {
                continue;
            }
            if b == b'\n' {
                if in_comment {
                    if let Some(count) = header_image_count(&header_prefix) {
                        return Ok(count);
                    }
                    header_prefix.clear();
                } else if saw_nonempty {
                    rows += 1;
                    cp.tick(1)?;
                }
                at_line_start = true;
                in_comment = false;
                saw_nonempty = false;
                continue;
            }
            if at_line_start {
                at_line_start = false;
                if b == b'#' {
                    in_comment = true;
                    header_prefix.clear();
                    header_prefix.push('#');
                    continue;
                }
                saw_nonempty = true;
                continue;
            }
            if in_comment {
                if header_prefix.len() < IMAGE_COUNT_HEADER_CAP {
                    // COLMAP headers are ASCII; lossy push keeps the cap byte-sized.
                    header_prefix.push(char::from(b));
                }
            } else {
                saw_nonempty = true;
            }
        }
    }

    // Final line without trailing newline (same as former read_line EOF).
    if in_comment {
        if let Some(count) = header_image_count(&header_prefix) {
            return Ok(count);
        }
    } else if saw_nonempty {
        rows += 1;
    }
    Ok(rows / 2)
}

/// Within 8192 bytes require `<document` and (`path=` for `.psx` / project, or
/// `<chunk` for `.xml`). Pin `ColmapParser.cpp:977-1005` — no literal `psx:`
/// namespace check. Bare `meta` / `<chunk/>` must not match.
fn looks_like_metashape(path: &Path, is_psx: bool) -> bool {
    let Some(bytes) = read_prefix(path, METASHAPE_SCAN_BYTES) else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes);
    if !head.contains("<document") {
        return false;
    }
    if is_psx {
        head.contains("path=")
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

/// Probe a dataset directory (auto-detect: Nerfstudio > COLMAP > Metashape).
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
        return Ok(DatasetProbe::Available {
            format: DatasetFormat::Nerfstudio,
            recon_dir: dataset_dir.to_path_buf(),
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
                // Prefer XML export as Available; .psx-only stays typed unavailable
                // (no zip runtime).
                let has_xml = std::fs::read_dir(dataset_dir)
                    .ok()
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|ent| {
                        let p = ent.path();
                        p.is_file()
                            && p.extension()
                                .and_then(|x| x.to_str())
                                .is_some_and(|x| x.eq_ignore_ascii_case("xml"))
                            && looks_like_metashape(&p, false)
                    });
                if has_xml {
                    Ok(DatasetProbe::Available {
                        format: DatasetFormat::Metashape,
                        recon_dir: dataset_dir.to_path_buf(),
                    })
                } else {
                    Ok(DatasetProbe::Unavailable {
                        format: DatasetFormat::Metashape,
                        reason: crate::metashape::psx_unsupported_reason().into(),
                    })
                }
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

        // Pin: .psx / project form is `<document path="..."/>` — no `psx:` required.
        let path_only = h.path().join("path_only.psx");
        std::fs::write(&path_only, b"<document path=\"scene.psz\"/>").unwrap();
        assert!(
            looks_like_metashape(&path_only, true),
            "<document path=...> must identify as Metashape"
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
            DatasetProbe::Available {
                format: DatasetFormat::Metashape,
                ..
            } => {}
            other => panic!("expected metashape available for XML export, got {other:?}"),
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

    #[test]
    fn huge_comment_line_stays_bounded_and_counts_data() {
        let h = TempHarness::new().unwrap();
        let dir = h.path().join("m");
        std::fs::create_dir_all(&dir).unwrap();
        // One oversized comment (no early header match past cap) + two image rows.
        let mut body = String::from("# ");
        body.push_str(&"x".repeat(IMAGE_COUNT_HEADER_CAP + 64 * 1024));
        body.push_str("\n0 1 0 0 0 0 0 0 1 a.png\n0 0 -1\n1 1 0 0 0 0 0 0 1 b.png\n0 0 -1\n");
        std::fs::write(dir.join("images.txt"), body).unwrap();
        assert_eq!(colmap_model_num_images(&dir), 2);
    }

    #[test]
    fn cancel_during_image_count_stream_observes_token() {
        use spirula_core::{CancelReason, ErrorCode};
        let h = TempHarness::new().unwrap();
        let dir = h.path().join("m");
        std::fs::create_dir_all(&dir).unwrap();
        // Larger than one read chunk so tick(n) fires on the first fill.
        let mut body = String::new();
        for i in 0..2_000 {
            body.push_str(&format!(
                "{i} 1 0 0 0 0 0 0 1 {}.png\n0 0 -1\n",
                "a".repeat(40)
            ));
        }
        std::fs::write(dir.join("images.txt"), body).unwrap();
        let cancel = spirula_core::CancelToken::new();
        assert!(cancel.request(CancelReason::User));
        let err = colmap_model_num_images_cancelable(&dir, Some(&cancel)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Canceled);
    }

    #[test]
    fn after_start_cancel_during_image_count_stream() {
        use spirula_core::{CancelReason, ErrorCode};
        use std::io;

        /// Controlled reader: cancel is armed only after the first chunk returns,
        /// so `poll_now` succeeds and the subsequent `tick` observes cancel.
        /// No sleep; no FIFO/`is_file` skip path.
        struct ArmCancelAfterFirstChunk {
            cancel: spirula_core::CancelToken,
            data: Vec<u8>,
            pos: usize,
            reads: usize,
        }

        impl Read for ArmCancelAfterFirstChunk {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.pos >= self.data.len() {
                    return Ok(0);
                }
                let n = IMAGE_COUNT_READ_CHUNK
                    .min(buf.len())
                    .min(self.data.len() - self.pos);
                buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
                self.pos += n;
                self.reads += 1;
                if self.reads == 1 {
                    assert!(self.cancel.request(CancelReason::User));
                }
                Ok(n)
            }
        }

        let mut body = String::new();
        for i in 0..4_000 {
            body.push_str(&format!(
                "{i} 1 0 0 0 0 0 0 1 {}.png\n0 0 -1\n",
                "a".repeat(40)
            ));
        }
        let cancel = spirula_core::CancelToken::new();
        let reader = ArmCancelAfterFirstChunk {
            cancel: cancel.clone(),
            data: body.into_bytes(),
            pos: 0,
            reads: 0,
        };
        let err =
            count_images_txt_stream(reader, Path::new("images.txt"), Some(&cancel)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Canceled);
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
