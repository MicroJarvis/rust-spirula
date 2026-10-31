//! Legacy checkpoint metadata reader (P3-F07).
//!
//! This module reads the reference `step-*.ckpt/{splat.ply,state.tar}` layout.
//! It deliberately does not publish new bundles or claim full training
//! recovery; P5-F05/P5-F07 own those behaviours.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use spirula_core::{CancelToken, ContextKey, DomainError, ErrorCode};

use crate::cancel_checkpoint::CancelCheckpoint;
use crate::colmap::budget::{MAX_COLMAP_FILE_BYTES, MAX_LOAD_MEMORY_BYTES, ParseMem, err_path};

const TAR_BLOCK: usize = 512;
const NPY_MAGIC: &[u8] = b"\x93NUMPY";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointClass {
    FullResume,
    ModelOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpyBlockMetadata {
    pub name: String,
    pub shape: Vec<usize>,
    pub payload_bytes: usize,
    pub item_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointStateMetadata {
    pub format_version: u64,
    pub full_resume: bool,
    pub step: u64,
    pub cur_num_splats: u64,
    pub max_num_splats: u64,
    pub num_sh: u64,
    pub sh_degree: u64,
    pub primitive: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointMetadata {
    pub checkpoint_dir: PathBuf,
    pub class: CheckpointClass,
    pub state: CheckpointStateMetadata,
    pub npy_blocks: Vec<NpyBlockMetadata>,
    pub splat_path: PathBuf,
}

fn checkpoint_error(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn incomplete(field: &str, message: impl Into<String>) -> DomainError {
    checkpoint_error(ErrorCode::CheckpointIncomplete, field, message)
}

fn corrupt(field: &str, message: impl Into<String>) -> DomainError {
    checkpoint_error(ErrorCode::CheckpointCorrupt, field, message)
}

fn safe_member(name: &str) -> Result<(), DomainError> {
    let p = Path::new(name);
    if name.is_empty() || name.contains('\\') || name.contains("//") || p.is_absolute() {
        return Err(corrupt(
            "tar_member",
            format!("unsafe checkpoint member `{name}`"),
        ));
    }
    for c in p.components() {
        if !matches!(c, Component::Normal(_)) {
            return Err(corrupt(
                "tar_member",
                format!("unsafe checkpoint member `{name}`"),
            ));
        }
    }
    Ok(())
}

fn octal(field: &[u8], name: &str) -> Result<usize, DomainError> {
    let text = field
        .iter()
        .copied()
        .take_while(|b| *b != 0 && *b != b' ')
        .filter(|b| !b.is_ascii_whitespace())
        .collect::<Vec<_>>();
    let s = std::str::from_utf8(&text).map_err(|_| corrupt("tar_size", "tar size is not UTF-8"))?;
    usize::from_str_radix(s.trim(), 8)
        .map_err(|_| corrupt("tar_size", format!("invalid tar size in `{name}`")))
}

fn verify_checksum(header: &[u8]) -> Result<(), DomainError> {
    let expected = octal(&header[148..156], "header")? as u64;
    let actual = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                b' '
            } else {
                *byte
            }
        })
        .map(u64::from)
        .sum::<u64>();
    if actual != expected {
        return Err(corrupt("tar_checksum", "ustar header checksum mismatch"));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct TarMember {
    name: String,
    data: Vec<u8>,
}

fn parse_tar(bytes: &[u8], cancel: Option<&CancelToken>) -> Result<Vec<TarMember>, DomainError> {
    if bytes.len() > MAX_COLMAP_FILE_BYTES as usize {
        return Err(checkpoint_error(
            ErrorCode::ValueOutOfRange,
            "state.tar",
            "state.tar exceeds byte budget",
        ));
    }
    if !bytes.len().is_multiple_of(TAR_BLOCK) {
        return Err(incomplete(
            "state.tar",
            "state.tar is truncated to a partial block",
        ));
    }
    let mut cp = CancelCheckpoint::new(cancel);
    let mut out = Vec::new();
    let mut off = 0usize;
    let mut zero_blocks = 0usize;
    while off + TAR_BLOCK <= bytes.len() {
        cp.poll_now()?;
        let header = &bytes[off..off + TAR_BLOCK];
        if header.iter().all(|b| *b == 0) {
            zero_blocks += 1;
            off += TAR_BLOCK;
            if zero_blocks == 2 {
                if bytes[off..].iter().any(|b| *b != 0) {
                    return Err(corrupt(
                        "tar_trailer",
                        "non-zero data follows the ustar trailer",
                    ));
                }
                if out.is_empty() {
                    return Err(incomplete("state.tar", "checkpoint tar has no members"));
                }
                return Ok(out);
            }
            continue;
        }
        if &header[257..263] != b"ustar\0" {
            return Err(corrupt(
                "tar_magic",
                "checkpoint is not a POSIX ustar archive",
            ));
        }
        verify_checksum(header)?;
        zero_blocks = 0;
        let end = header[..100].iter().position(|b| *b == 0).unwrap_or(100);
        let name = std::str::from_utf8(&header[..end])
            .map_err(|_| corrupt("tar_member", "tar member name is not UTF-8"))?
            .to_owned();
        safe_member(&name)?;
        if header[156] != 0 && header[156] != b'0' {
            return Err(corrupt(
                "tar_member",
                format!("checkpoint member `{name}` is not a regular file"),
            ));
        }
        if out.iter().any(|m: &TarMember| m.name == name) {
            return Err(corrupt(
                "tar_member",
                format!("duplicate checkpoint member `{name}`"),
            ));
        }
        let size = octal(&header[124..136], &name)?;
        let start = off
            .checked_add(TAR_BLOCK)
            .ok_or_else(|| incomplete("state.tar", "tar offset overflow"))?;
        let end_data = start
            .checked_add(size)
            .ok_or_else(|| incomplete("state.tar", "tar member overflow"))?;
        if end_data > bytes.len() {
            return Err(incomplete(
                "state.tar",
                format!("member `{name}` is truncated"),
            ));
        }
        let padded = size
            .div_ceil(TAR_BLOCK)
            .checked_mul(TAR_BLOCK)
            .ok_or_else(|| incomplete("state.tar", "tar padding overflow"))?;
        let next = start
            .checked_add(padded)
            .ok_or_else(|| incomplete("state.tar", "tar offset overflow"))?;
        if next > bytes.len() {
            return Err(incomplete(
                "state.tar",
                format!("member `{name}` padding is truncated"),
            ));
        }
        out.push(TarMember {
            name,
            data: bytes[start..end_data].to_vec(),
        });
        off = next;
    }
    if zero_blocks < 2 {
        return Err(incomplete(
            "state.tar",
            "checkpoint tar has no complete trailer",
        ));
    }
    if out.is_empty() {
        return Err(incomplete("state.tar", "checkpoint tar has no members"));
    }
    Ok(out)
}

fn parse_shape(text: &str) -> Result<Vec<usize>, DomainError> {
    let mut shape = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        shape.push(
            part.parse::<usize>()
                .map_err(|_| corrupt("npy_shape", "invalid NPY shape"))?,
        );
    }
    Ok(shape)
}

fn header_value<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("'{key}'");
    let start = header.find(&marker)? + marker.len();
    let rest = header[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    Some(rest)
}

fn quoted_value(value: &str) -> Option<&str> {
    let quote = value.as_bytes().first().copied()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let end = value[1..].find(quote as char)? + 1;
    Some(&value[1..end])
}

fn parse_npy(
    name: &str,
    bytes: &[u8],
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<NpyBlockMetadata, DomainError> {
    if bytes.len() < 10 || !bytes.starts_with(NPY_MAGIC) {
        return Err(corrupt("npy", format!("`{name}` has invalid NPY magic")));
    }
    let major = bytes[6];
    let header_len = if major == 1 {
        u16::from_le_bytes([bytes[8], bytes[9]]) as usize
    } else if (2..=3).contains(&major) && bytes.len() >= 12 {
        u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize
    } else {
        return Err(checkpoint_error(
            ErrorCode::UnsupportedVersion,
            "npy_version",
            format!("unsupported NPY version {major}"),
        ));
    };
    let header_start: usize = if major == 1 { 10 } else { 12 };
    let header_end = header_start
        .checked_add(header_len)
        .ok_or_else(|| incomplete("npy_header", "NPY header overflow"))?;
    if header_end > bytes.len() {
        return Err(incomplete(
            "npy_header",
            format!("`{name}` header is truncated"),
        ));
    }
    let header = std::str::from_utf8(&bytes[header_start..header_end])
        .map_err(|_| corrupt("npy_header", "NPY header is not UTF-8"))?;
    let descr = header_value(header, "descr")
        .and_then(quoted_value)
        .ok_or_else(|| corrupt("npy_descr", "NPY descr is missing"))?;
    if descr != "<f4" {
        return Err(corrupt(
            "npy_descr",
            format!("unsupported NPY descr `{descr}`"),
        ));
    }
    let fortran = header_value(header, "fortran_order")
        .and_then(|value| value.split([',', '}']).next())
        .map(str::trim)
        .ok_or_else(|| corrupt("npy_order", "NPY fortran_order is missing"))?;
    if fortran != "False" {
        return Err(corrupt("npy_order", "NPY must use C order"));
    }
    let shape_value = header_value(header, "shape")
        .ok_or_else(|| corrupt("npy_shape", "NPY shape is missing"))?;
    let shape_text = shape_value
        .strip_prefix('(')
        .and_then(|s| s.split(')').next())
        .ok_or_else(|| corrupt("npy_shape", "NPY shape is missing"))?;
    let shape = parse_shape(shape_text)?;
    let count = shape
        .iter()
        .try_fold(1usize, |a, b| a.checked_mul(*b))
        .ok_or_else(|| corrupt("npy_shape", "NPY shape overflows"))?;
    let payload_bytes = count
        .checked_mul(4)
        .ok_or_else(|| corrupt("npy_payload", "NPY payload size overflows"))?;
    if bytes.len() - header_end != payload_bytes {
        let code = if bytes.len() - header_end < payload_bytes {
            ErrorCode::CheckpointIncomplete
        } else {
            ErrorCode::CheckpointCorrupt
        };
        return Err(checkpoint_error(
            code,
            "npy_payload",
            format!("`{name}` payload length does not match shape"),
        ));
    }
    let mut cp = CancelCheckpoint::new(cancel);
    for chunk in bytes[header_end..].as_chunks::<4>().0 {
        cp.tick(1)?;
        if !f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]).is_finite() {
            return Err(corrupt(
                "npy_payload",
                format!("`{name}` contains a non-finite value"),
            ));
        }
    }
    mem.charge(payload_bytes as u64, "checkpoint_npy")?;
    Ok(NpyBlockMetadata {
        name: name.to_owned(),
        shape,
        payload_bytes,
        item_count: count,
    })
}

fn required_u64(state: &Value, key: &str) -> Result<u64, DomainError> {
    state
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| corrupt(key, format!("state.json missing `{key}`")))
}

pub fn read_checkpoint_metadata(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<CheckpointMetadata, DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
    cp.poll_now()?;
    let state_tar = path.join("state.tar");
    let bytes = std::fs::read(&state_tar).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            incomplete("state.tar", "checkpoint is missing state.tar")
        } else {
            err_path(
                ErrorCode::ReadFailed,
                &state_tar.display().to_string(),
                format!("cannot read state.tar: {e}"),
            )
        }
    })?;
    let mut mem = ParseMem::with_cap(MAX_LOAD_MEMORY_BYTES);
    mem.charge(bytes.len() as u64, "state.tar")?;
    let members = parse_tar(&bytes, cancel)?;
    let state_bytes = members
        .iter()
        .find(|m| m.name == "state.json")
        .map(|m| m.data.as_slice())
        .ok_or_else(|| incomplete("state.json", "checkpoint is missing state.json"))?;
    let state: Value = serde_json::from_slice(state_bytes)
        .map_err(|e| corrupt("state.json", format!("invalid state.json: {e}")))?;
    let version = required_u64(&state, "format_version")?;
    if version != 1 {
        return Err(checkpoint_error(
            ErrorCode::UnsupportedVersion,
            "format_version",
            format!("unsupported checkpoint format version {version}"),
        ));
    }
    let full_resume = state
        .get("full_resume")
        .and_then(Value::as_u64)
        .ok_or_else(|| corrupt("full_resume", "state.json missing `full_resume`"))?;
    if full_resume > 1 {
        return Err(corrupt("full_resume", "full_resume must be 0 or 1"));
    }
    let meta = CheckpointStateMetadata {
        format_version: version,
        full_resume: full_resume == 1,
        step: required_u64(&state, "step")?,
        cur_num_splats: required_u64(&state, "cur_num_splats")?,
        max_num_splats: required_u64(&state, "max_num_splats")?,
        num_sh: required_u64(&state, "num_sh")?,
        sh_degree: required_u64(&state, "sh_degree")?,
        primitive: state
            .get("primitive")
            .and_then(Value::as_str)
            .ok_or_else(|| corrupt("primitive", "state.json missing `primitive`"))?
            .to_owned(),
    };
    if meta.cur_num_splats > meta.max_num_splats {
        return Err(corrupt("cur_num_splats", "current splat count exceeds max"));
    }
    let mut npy = Vec::new();
    for member in &members {
        if member.name.ends_with(".npy") {
            npy.push(parse_npy(&member.name, &member.data, &mut mem, cancel)?);
        }
    }
    if npy.is_empty() {
        return Err(incomplete("npy", "checkpoint contains no NPY state blocks"));
    }
    for required in ["world.means.npy", "world.opacities.npy"] {
        let block = npy.iter().find(|block| block.name == required);
        if block.is_none() {
            return Err(incomplete(
                "npy",
                format!("checkpoint is missing `{required}`"),
            ));
        }
        let block = block.expect("checked above");
        let first_dim = block
            .shape
            .first()
            .and_then(|value| u64::try_from(*value).ok());
        if first_dim != Some(meta.cur_num_splats) {
            return Err(corrupt(
                "npy_shape",
                format!("`{required}` count does not match cur_num_splats"),
            ));
        }
    }
    let splat_path = path.join("splat.ply");
    if !splat_path.is_file() {
        return Err(incomplete("splat.ply", "checkpoint is missing splat.ply"));
    }
    Ok(CheckpointMetadata {
        checkpoint_dir: path.to_path_buf(),
        class: if meta.full_resume {
            CheckpointClass::FullResume
        } else {
            CheckpointClass::ModelOnly
        },
        state: meta,
        npy_blocks: npy,
        splat_path,
    })
}

pub fn require_full_resume(metadata: &CheckpointMetadata) -> Result<(), DomainError> {
    if metadata.class == CheckpointClass::FullResume {
        Ok(())
    } else {
        Err(checkpoint_error(
            ErrorCode::CheckpointNotResumable,
            "full_resume",
            "checkpoint contains model state only and cannot resume training",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npy(major: u8, shape: &str) -> Vec<u8> {
        let dict = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({shape}), }}");
        let start = if major == 1 { 10 } else { 12 };
        let padding = (16 - (start + dict.len() + 1) % 16) % 16;
        let header_len = dict.len() + padding + 1;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(NPY_MAGIC);
        bytes.extend_from_slice(&[major, 0]);
        if major == 1 {
            bytes.extend_from_slice(&(header_len as u16).to_le_bytes());
        } else {
            bytes.extend_from_slice(&(header_len as u32).to_le_bytes());
        }
        bytes.extend_from_slice(dict.as_bytes());
        bytes.extend(std::iter::repeat_n(b' ', padding));
        bytes.push(b'\n');
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn accepts_npy_v1_v2_v3_and_checks_shape() {
        for version in [1, 2, 3] {
            let mut mem = ParseMem::with_cap(MAX_LOAD_MEMORY_BYTES);
            let block = parse_npy("test.npy", &npy(version, "3"), &mut mem, None).unwrap();
            assert_eq!(block.shape, vec![3]);
            assert_eq!(block.payload_bytes, 12);
        }
    }

    #[test]
    fn rejects_bad_shape_and_enforces_memory_budget() {
        let mut mem = ParseMem::with_cap(MAX_LOAD_MEMORY_BYTES);
        assert_eq!(
            parse_npy("test.npy", &npy(1, "bad"), &mut mem, None)
                .unwrap_err()
                .code(),
            ErrorCode::CheckpointCorrupt
        );
        let mut mem = ParseMem::with_cap(0);
        assert_eq!(
            parse_npy("test.npy", &npy(1, "3"), &mut mem, None)
                .unwrap_err()
                .code(),
            ErrorCode::ValueOutOfRange
        );
    }
}
