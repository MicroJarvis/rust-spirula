//! Content hashing helpers shared by fixtures and performance reports.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use spirula_core::{ContextKey, DomainError, ErrorCode};

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Hash a file's full contents. Missing or unreadable paths map to domain errors.
pub fn sha256_file(path: &Path) -> Result<String, DomainError> {
    let bytes = fs::read(path).map_err(|error| {
        DomainError::try_new(ErrorCode::ReadFailed, "failed to read file for hashing")
            .expect("message")
            .with_context(ContextKey::Path, path.display().to_string())
            .expect("path")
            .with_source(error)
    })?;
    Ok(sha256_hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_hash_is_stable() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
