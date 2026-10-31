//! Parent-owned real CLI review. Bounded subprocess waits; input must affect PNG.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "spirula-f06-review-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let mut ply = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\n".to_vec();
        for name in [
            "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0",
            "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
        ] {
            ply.extend_from_slice(format!("property float {name}\n").as_bytes());
        }
        ply.extend_from_slice(b"end_header\n");
        for value in [
            0f32, 0., -2., 0., 0., 0., -2., 0., 2., 0., -30., -30., -30., 1., 0., 0., 0.,
        ] {
            ply.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(path.join("scene.ply"), ply).unwrap();
        Self(path)
    }
    fn request(&self, cx: f64, version: u32) -> PathBuf {
        let path = self.0.join("preview.json");
        fs::write(&path,format!(r#"{{"schema":"spirula-preview-request","version":{version},"splat_ply":"scene.ply","camera":{{"model":"PINHOLE","width":3,"height":2,"fx":1,"fy":1,"cx":{cx},"cy":0.5,"c2w":{{"rotation_row_major":[1,0,0,0,1,0,0,0,1],"translation":[0,0,0]}}}},"background":[0.1,0.2,0.3],"thresholds":{{"alpha_cutoff":0.1,"early_out_transmittance":0.0001}}}}"#)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_spirula"))
        .current_dir(cwd)
        .args(args)
        .env("SPIRULA_ERROR_CODES", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("preview subprocess exceeded 30s deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

#[test]
fn cli_preview_writes_real_opaque_png_from_relative_request_and_camera_changes() {
    let fixture = Fixture::new();
    fixture.request(0.5, 1);
    let first = run(
        &fixture.0,
        &["preview", "--request", "preview.json", "--out", "first.png"],
    );
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stderr.is_empty());
    let png = fs::read(fixture.0.join("first.png")).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 3);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 2);
    assert_eq!((png[24], png[25]), (8, 2), "opaque RGB8 PNG required");
    fixture.request(1.5, 1);
    let second = run(
        &fixture.0,
        &[
            "preview",
            "--request",
            "preview.json",
            "--out",
            "second.png",
        ],
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_ne!(
        fs::read(fixture.0.join("second.png")).unwrap(),
        png,
        "camera change must affect real render output"
    );
}

#[test]
fn cli_preview_failure_has_one_error_preserves_destination_and_can_retry() {
    let fixture = Fixture::new();
    fixture.request(0.5, 2);
    let output = fixture.0.join("out.png");
    fs::write(&output, b"prior-output").unwrap();
    let invalid = run(
        &fixture.0,
        &["preview", "--request", "preview.json", "--out", "out.png"],
    );
    assert_eq!(invalid.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert_eq!(
        stderr.lines().filter(|l| l.starts_with("error:")).count(),
        1,
        "{stderr}"
    );
    assert!(
        stderr.contains("SP-"),
        "stable error code missing: {stderr}"
    );
    assert_eq!(fs::read(&output).unwrap(), b"prior-output");
    fixture.request(0.5, 1);
    let retry = run(
        &fixture.0,
        &["preview", "--request", "preview.json", "--out", "out.png"],
    );
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert_ne!(fs::read(&output).unwrap(), b"prior-output");
    let usage = run(&fixture.0, &["preview", "--not-a-preview-option"]);
    assert_eq!(usage.status.code(), Some(2));
}

#[cfg(unix)]
#[test]
#[allow(unsafe_code)] // Deliver SIGINT only to the child process owned by this test.
fn cli_committed_preview_keeps_success_when_sigint_arrives_before_terminal_observation() {
    let fixture = Fixture::new();
    fixture.request(0.5, 1);
    let output = fixture.0.join("committed.png");
    let mut child = Command::new(env!("CARGO_BIN_EXE_spirula"))
        .current_dir(&fixture.0)
        .args([
            "preview",
            "--request",
            "preview.json",
            "--out",
            "committed.png",
        ])
        // Existing pressure seam delays terminal observation through query/replay.
        // Signal is synchronized to the physical rename, never a fixed sleep.
        .env("SPIRULA_TEST_TINY_NOTIFY", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if output.exists() {
            // This process belongs to the test; the file proves commit happened.
            assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
            break;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "exited before publication"
        );
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("preview did not publish before deadline");
        }
        std::thread::sleep(Duration::from_micros(100));
    }
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("committed preview did not terminate before deadline");
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let stdout = String::from_utf8(out.join().unwrap()).unwrap();
    let stderr = String::from_utf8(err.join().unwrap()).unwrap();
    assert_eq!(status.code(), Some(0), "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("completed ok"), "{stdout}");
    assert!(!stdout.contains("completed canceled"), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(&fs::read(output).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
}
