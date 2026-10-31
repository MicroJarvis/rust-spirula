//! Independent package proof: decoded pixels, sample integrity, legacy schema.
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value;
use sha2::{Digest, Sha256};
use spirula_io::temp_harness::TempHarness;
use spirula_xtask::{
    AssembleMeta, assemble_package_from_bin_dir, check_package, run_package_isolated,
    workspace_root,
};

fn bins() -> &'static PathBuf {
    static BINS: OnceLock<PathBuf> = OnceLock::new();
    BINS.get_or_init(|| {
        let root = workspace_root();
        let result = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args([
                "build",
                "--locked",
                "-p",
                "spirula-cli",
                "-p",
                "spirula-gpu",
            ])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(result.success());
        root.join("target/debug")
    })
}

fn package(harness: &TempHarness) -> PathBuf {
    let out = harness.path().join("review package with spaces");
    assemble_package_from_bin_dir(
        bins(),
        &out,
        &AssembleMeta {
            revision: "review-dirty-dev-not-clean-ref-proof".into(),
            version: "0.1.0".into(),
            profile: "dev".into(),
        },
    )
    .unwrap();
    out
}

#[test]
fn isolated_package_decodes_full_preview_golden_and_rejects_sample_tampering() {
    let harness = TempHarness::new().unwrap();
    let out = package(&harness);
    let manifest = check_package(&out).unwrap();
    assert_eq!(manifest["samples"].as_array().unwrap().len(), 2);
    let sample = out.join("sample/preview/scene.ply");
    let original = fs::read(&sample).unwrap();
    let mut changed = original.clone();
    *changed.last_mut().unwrap() ^= 1;
    fs::write(&sample, changed).unwrap();
    assert!(check_package(&out).unwrap_err().contains("sha256 mismatch"));
    fs::write(&sample, &original).unwrap();
    #[cfg(unix)]
    {
        fs::remove_file(&sample).unwrap();
        std::os::unix::fs::symlink(out.join("NOTICE"), &sample).unwrap();
        assert!(check_package(&out).unwrap_err().contains("symlink"));
        fs::remove_file(&sample).unwrap();
        fs::write(&sample, &original).unwrap();
    }
    let extra = out.join("sample/preview/unlisted.json");
    fs::write(&extra, b"{}").unwrap();
    assert!(check_package(&out).is_err());
    fs::remove_file(extra).unwrap();
    check_package(&out).unwrap();

    let report = run_package_isolated(&out, &harness.path().join("source free isolate")).unwrap();
    let (width, height, rgb) =
        spirula_io::image_probe::decode_rgb8(&report.cwd.join("preview-out.png")).unwrap();
    assert_eq!((width, height), (3, 2));
    // Independent scalar reference from pinned PLY floats: covariance=.3+exp(2s)/4,
    // alpha=sigmoid(opacity)*exp(-(x*x+y*y)/(2*cov)), cutoff=.1;
    // SH0=.5+C0*dc, C0=.28209479177387814; linear clamp+lround255.
    // No CPU/GPU renderer output is used to construct these expected bytes.
    assert_eq!(
        rgb,
        [
            253, 1, 1, 71, 41, 61, 26, 51, 77, 71, 41, 61, 26, 51, 77, 26, 51, 77
        ]
    );

    // A self-consistent hash manifest is not proof of PNG completeness. Replace
    // only our temporary executable with malformed/wrong-format output. The
    // isolated gate must reject it despite success exit codes and matching RGB.
    #[cfg(unix)]
    {
        let mut header = [0u8; 33];
        header[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        header[8..12].copy_from_slice(&13u32.to_be_bytes());
        header[12..16].copy_from_slice(b"IHDR");
        header[16..20].copy_from_slice(&3u32.to_be_bytes());
        header[20..24].copy_from_slice(&2u32.to_be_bytes());
        header[24] = 8;
        header[25] = 2;
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&78u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&3u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        bmp[34..38].copy_from_slice(&24u32.to_le_bytes());
        for row in rgb.as_chunks::<9>().0.iter().rev() {
            for pixel in row.as_chunks::<3>().0 {
                bmp.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            bmp.extend_from_slice(&[0; 3]);
        }
        assert_eq!(
            spirula_io::image_probe::decode_rgb8_bytes(&bmp).unwrap().2,
            rgb
        );
        for (label, payload) in [
            ("IHDR-only PNG", header.to_vec()),
            ("BMP instead of PNG", bmp),
        ] {
            let escaped: String = payload.iter().map(|b| format!("\\{b:03o}")).collect();
            let script = format!(
                "#!/bin/sh\nif [ \"$1\" = preview ]; then printf '{escaped}' > \"$5\"; fi\nexit 0\n"
            );
            fs::write(out.join("bin/spirula"), script.as_bytes()).unwrap();
            let manifest_path = out.join("MANIFEST.json");
            let mut manifest: Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
            let binary = manifest["binaries"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|binary| binary["name"] == "spirula")
                .unwrap();
            binary["sha256"] = format!("{:x}", Sha256::digest(script.as_bytes())).into();
            binary["size_bytes"] = (script.len() as u64).into();
            fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&manifest).unwrap(),
            )
            .unwrap();
            check_package(&out).expect("hash/layout checks of the test stub");
            assert!(
                run_package_isolated(&out, &harness.path().join(label)).is_err(),
                "{label} must not satisfy the real isolated preview gate"
            );
        }
    }
}

#[test]
fn previously_valid_p2_f07_schema1_package_remains_readable() {
    let harness = TempHarness::new().unwrap();
    let out = package(&harness);
    let path = out.join("MANIFEST.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["schema_version"] = 1.into();
    manifest.as_object_mut().unwrap().remove("samples");
    fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    fs::remove_dir_all(out.join("sample")).unwrap();
    // The old documented v1 contract has no samples[] or sample/ directory.
    // Enriching the new package must preserve reading this established format.
    check_package(&out).expect("legacy P2-F07 schema1 must remain readable");
}
