use std::path::{Path, PathBuf};

use spirula_core::{CancelReason, CancelToken, ErrorCode};
use spirula_io::{CheckpointClass, TempHarness, read_checkpoint_metadata, require_full_resume};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn fixture() -> PathBuf {
    root().join("fixtures/checkpoints/tiny-run/step-000000001.ckpt")
}

fn copy_checkpoint(h: &TempHarness) -> PathBuf {
    let dst = h.path().join("step-000000001.ckpt");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::copy(fixture().join("state.tar"), dst.join("state.tar")).unwrap();
    std::fs::copy(fixture().join("splat.ply"), dst.join("splat.ply")).unwrap();
    dst
}

fn replace_once(path: &Path, from: &[u8], to: &[u8]) {
    let mut bytes = std::fs::read(path).unwrap();
    let at = bytes.windows(from.len()).position(|w| w == from).unwrap();
    assert_eq!(from.len(), to.len());
    bytes[at..at + to.len()].copy_from_slice(to);
    std::fs::write(path, bytes).unwrap();
}

fn rename_member(path: &Path, old: &str, new: &str) {
    assert!(new.len() <= 100);
    let mut bytes = std::fs::read(path).unwrap();
    for block in bytes.as_chunks_mut::<512>().0 {
        let end = block[..100]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(100);
        if &block[..end] == old.as_bytes() {
            block[..100].fill(0);
            block[..new.len()].copy_from_slice(new.as_bytes());
            block[148..156].fill(b' ');
            let checksum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
            let text = format!("{checksum:06o}");
            block[148..154].copy_from_slice(text.as_bytes());
            block[154] = 0;
            block[155] = b' ';
            std::fs::write(path, bytes).unwrap();
            return;
        }
    }
    panic!("member {old} not found");
}

fn patch_first_npy_value(path: &Path, value: [u8; 4]) {
    let mut bytes = std::fs::read(path).unwrap();
    let at = bytes
        .windows(6)
        .position(|window| window == b"\x93NUMPY")
        .unwrap();
    let header_len = u16::from_le_bytes([bytes[at + 8], bytes[at + 9]]) as usize;
    let payload = at + 10 + header_len;
    bytes[payload..payload + 4].copy_from_slice(&value);
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn reads_fixture_metadata_and_npy_blocks() {
    let meta = read_checkpoint_metadata(&fixture(), None).unwrap();
    assert_eq!(meta.class, CheckpointClass::FullResume);
    assert_eq!(meta.state.step, 1);
    assert_eq!(meta.state.cur_num_splats, 3);
    assert_eq!(meta.state.max_num_splats, 8);
    assert_eq!(meta.npy_blocks.len(), 2);
    assert_eq!(meta.npy_blocks[0].payload_bytes, 36);
    assert_eq!(meta.splat_path, fixture().join("splat.ply"));
    require_full_resume(&meta).unwrap();
}

#[test]
fn model_only_is_classified_and_refused_for_resume() {
    let h = TempHarness::new().unwrap();
    let path = copy_checkpoint(&h);
    replace_once(
        &path.join("state.tar"),
        br#""full_resume": 1"#,
        br#""full_resume": 0"#,
    );
    let meta = read_checkpoint_metadata(&path, None).unwrap();
    assert_eq!(meta.class, CheckpointClass::ModelOnly);
    assert_eq!(
        require_full_resume(&meta).unwrap_err().code(),
        ErrorCode::CheckpointNotResumable
    );
}

#[test]
fn unknown_version_is_stable() {
    let h = TempHarness::new().unwrap();
    let path = copy_checkpoint(&h);
    replace_once(
        &path.join("state.tar"),
        br#""format_version": 1"#,
        br#""format_version": 2"#,
    );
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::UnsupportedVersion
    );
}

#[test]
fn missing_required_block_and_truncation_are_incomplete() {
    let h = TempHarness::new().unwrap();
    let path = copy_checkpoint(&h);
    rename_member(
        &path.join("state.tar"),
        "world.means.npy",
        "world.means.np_",
    );
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::CheckpointIncomplete
    );

    let path = copy_checkpoint(&h);
    let tar = path.join("state.tar");
    let mut bytes = std::fs::read(&tar).unwrap();
    let trailer = bytes
        .as_chunks::<512>()
        .0
        .iter()
        .position(|block| block.iter().all(|byte| *byte == 0))
        .unwrap();
    bytes.truncate(trailer * 512);
    std::fs::write(&tar, bytes).unwrap();
    let err = read_checkpoint_metadata(&path, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::CheckpointIncomplete);
}

#[test]
fn cancellation_is_observed_before_read() {
    let token = CancelToken::new();
    token.request(CancelReason::User);
    assert_eq!(
        read_checkpoint_metadata(&fixture(), Some(&token))
            .unwrap_err()
            .code(),
        ErrorCode::Canceled
    );
}

#[test]
fn unsafe_and_duplicate_members_are_corrupt() {
    let h = TempHarness::new().unwrap();
    let path = copy_checkpoint(&h);
    rename_member(&path.join("state.tar"), "world.means.npy", "../means.npy");
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::CheckpointCorrupt
    );

    let path = copy_checkpoint(&h);
    rename_member(&path.join("state.tar"), "world.means.npy", "state.json");
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::CheckpointCorrupt
    );
}

#[test]
fn nonfinite_payload_and_count_mismatch_are_corrupt() {
    let h = TempHarness::new().unwrap();
    let path = copy_checkpoint(&h);
    patch_first_npy_value(&path.join("state.tar"), f32::NAN.to_le_bytes());
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::CheckpointCorrupt
    );

    let path = copy_checkpoint(&h);
    replace_once(
        &path.join("state.tar"),
        br#""cur_num_splats": 3"#,
        br#""cur_num_splats": 4"#,
    );
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::CheckpointCorrupt
    );
}

#[test]
fn ply_only_checkpoint_is_incomplete() {
    let h = TempHarness::new().unwrap();
    let path = h.path().join("model-only.ckpt");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::copy(fixture().join("splat.ply"), path.join("splat.ply")).unwrap();
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::CheckpointIncomplete
    );
}

#[test]
fn bad_tar_checksum_is_corrupt() {
    let h = TempHarness::new().unwrap();
    let path = copy_checkpoint(&h);
    let tar = path.join("state.tar");
    let mut bytes = std::fs::read(&tar).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&tar, bytes).unwrap();
    assert_eq!(
        read_checkpoint_metadata(&path, None).unwrap_err().code(),
        ErrorCode::CheckpointCorrupt
    );
}
