use std::fs;
use std::io::Write;
use std::path::Path;

use crate::error::TaprootError;

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), TaprootError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() && parent != Path::new(".") {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| TaprootError::Io(e.error))?;
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub(crate) fn validate_non_empty(field: &str, value: &str) -> Result<(), TaprootError> {
    if value.trim().is_empty() {
        return Err(TaprootError::InvalidKey(format!(
            "{field} must be non-empty"
        )));
    }
    if value.len() > 256 {
        return Err(TaprootError::InvalidKey(format!(
            "{field} too long (max 256)"
        )));
    }
    if value.contains('\0') || value.contains('\\') {
        return Err(TaprootError::InvalidKey(format!(
            "{field} must not contain null byte or backslash"
        )));
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(TaprootError::InvalidKey(format!(
            "{field} must not contain newline"
        )));
    }
    if value == "." || value == ".." {
        return Err(TaprootError::InvalidKey(format!(
            "{field} must not be '.' or '..'"
        )));
    }
    if value.starts_with('/') || value.ends_with('/') {
        return Err(TaprootError::InvalidKey(format!(
            "{field} must not start or end with '/'"
        )));
    }
    if value.contains("//") {
        return Err(TaprootError::InvalidKey(format!(
            "{field} must not contain '//'"
        )));
    }
    for seg in value.split('/') {
        if seg == "." || seg == ".." {
            return Err(TaprootError::InvalidKey(format!(
                "{field} segment must not be '.' or '..'"
            )));
        }
        if seg.is_empty() && value.contains('/') {
            return Err(TaprootError::InvalidKey(format!(
                "{field} contains empty segment"
            )));
        }
    }
    Ok(())
}
