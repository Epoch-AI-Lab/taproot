use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::engine::StateEngine;
use crate::error::TaprootError;
use crate::state::SignedState;

/// Local content-addressed registry for signed states.
///
/// Layout (root is `.taproot/registry`):
/// - `objects/<hash>.json`  -> SignedState pretty JSON
/// - `refs/<sanitized_repo>/<sanitized_branch>` -> text file containing hash
///
/// Sanitization: `/` is encoded as `%2F`, `%` as `%25`, so
/// `org/myapp` + `feat/foo` => `refs/org%2Fmyapp/feat%2Ffoo`.
/// This avoids the old `__` collision where `a/b` and `a__b` mapped to the same path.
pub struct Registry {
    root: PathBuf,
}

impl Registry {
    /// Create a registry handle without touching the filesystem.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Initialise registry directories (`objects/` + `refs/`).
    pub fn init(root: &Path) -> Result<Self, TaprootError> {
        let r = Self::new(root);
        fs::create_dir_all(r.objects_dir())?;
        fs::create_dir_all(r.refs_dir())?;
        tracing::info!(?root, "registry init");
        Ok(r)
    }

    /// Open existing registry, ensuring base dirs exist (idempotent).
    pub fn open(root: &Path) -> Result<Self, TaprootError> {
        Self::init(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn refs_dir(&self) -> PathBuf {
        self.root.join("refs")
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        self.objects_dir().join(format!("{hash}.json"))
    }

    fn ref_path(&self, repo: &str, branch: &str) -> Result<PathBuf, TaprootError> {
        validate_non_empty("repo", repo)?;
        validate_non_empty("branch", branch)?;
        let repo_s = sanitize(repo);
        let branch_s = sanitize(branch);
        Ok(self.refs_dir().join(repo_s).join(branch_s))
    }

    /// Push a signed state: verify, persist object, update ref.
    /// Returns the hash on success.
    pub fn push(&self, signed: &SignedState) -> Result<String, TaprootError> {
        // Validate + verify before any IO.
        StateEngine::verify(signed)?;
        let computed = StateEngine::hash(&signed.state)?;
        if computed != signed.hash {
            return Err(TaprootError::HashMismatch {
                expected: signed.hash.clone(),
                got: computed,
            });
        }
        validate_non_empty("repo", &signed.state.base.repo)?;
        validate_non_empty("branch", &signed.state.base.branch)?;
        validate_hash(&signed.hash)?;

        // Ensure dirs exist.
        fs::create_dir_all(self.objects_dir())?;
        fs::create_dir_all(self.refs_dir())?;

        // Write object atomically if not already present.
        let obj_path = self.object_path(&signed.hash);
        if !obj_path.exists() {
            let bytes = serde_json::to_vec_pretty(signed)?;
            atomic_write(&obj_path, &bytes)?;
            tracing::info!(hash=%signed.hash, ?obj_path, "registry object written");
        } else {
            // Verify existing object matches (defensive).
            let existing = self.pull(&signed.hash)?;
            if existing != *signed {
                tracing::warn!(hash=%signed.hash, "registry object exists with different content");
            }
        }

        // Update ref atomically.
        let ref_path = self.ref_path(&signed.state.base.repo, &signed.state.base.branch)?;
        if let Some(parent) = ref_path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&ref_path, signed.hash.as_bytes())?;
        tracing::info!(
            repo=%signed.state.base.repo,
            branch=%signed.state.base.branch,
            hash=%signed.hash,
            "registry ref updated"
        );

        Ok(signed.hash.clone())
    }

    /// Pull an object by hash. Verifies signature and hash.
    pub fn pull(&self, hash: &str) -> Result<SignedState, TaprootError> {
        validate_hash(hash)?;
        let path = self.object_path(hash);
        if !path.exists() {
            return Err(TaprootError::ObjectNotFound(hash.to_string()));
        }
        let bytes = fs::read(&path)?;
        let signed: SignedState = serde_json::from_slice(&bytes)?;
        StateEngine::verify(&signed)?;
        if signed.hash != hash {
            return Err(TaprootError::HashMismatch {
                expected: hash.to_string(),
                got: signed.hash.clone(),
            });
        }
        Ok(signed)
    }

    /// Resolve a ref to a hash, if present.
    pub fn resolve_ref(&self, repo: &str, branch: &str) -> Result<Option<String>, TaprootError> {
        let path = self.ref_path(repo, branch)?;
        if !path.exists() {
            return Ok(None);
        }
        // Ensure it's a file, not a directory.
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() {
            return Err(TaprootError::InvalidKey(format!(
                "ref path is not a file: {}",
                path.display()
            )));
        }
        let content = fs::read_to_string(&path)?;
        let hash = content.trim().to_string();
        if hash.is_empty() {
            return Ok(None);
        }
        validate_hash(&hash)?;
        Ok(Some(hash))
    }

    /// List branches for a repo. Returns sorted (branch, hash) pairs.
    /// Branch names are de-sanitized (`%2F` -> `/`).
    pub fn list(&self, repo: &str) -> Result<Vec<(String, String)>, TaprootError> {
        validate_non_empty("repo", repo)?;
        let repo_s = sanitize(repo);
        let dir = self.refs_dir().join(repo_s);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if !ft.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            let branch = desanitize(&file_name);
            // Validate branch round-trips.
            validate_non_empty("branch", &branch)?;
            let hash = fs::read_to_string(entry.path())?.trim().to_string();
            if hash.is_empty() {
                continue;
            }
            // Skip invalid hashes rather than failing whole list.
            if validate_hash(&hash).is_err() {
                tracing::warn!(?hash, branch=%branch, "skipping ref with invalid hash");
                eprintln!("warn: skipping bad ref {branch} with invalid hash {hash}");
                continue;
            }
            out.push((branch, hash));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Log for a repo/branch. Currently returns the single SignedState
    /// pointed to by the ref, if any (no history chain yet).
    pub fn log(&self, repo: &str, branch: &str) -> Result<Vec<SignedState>, TaprootError> {
        match self.resolve_ref(repo, branch)? {
            Some(hash) => {
                let signed = self.pull(&hash)?;
                Ok(vec![signed])
            }
            None => Ok(Vec::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn sanitize(s: &str) -> String {
    // Order matters: escape % first, then /
    s.replace('%', "%25").replace('/', "%2F")
}

pub(crate) fn desanitize(s: &str) -> String {
    // Reverse: %2F -> /, then %25 -> %
    s.replace("%2F", "/").replace("%25", "%")
}

fn validate_non_empty(field: &str, value: &str) -> Result<(), TaprootError> {
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

fn validate_hash(hash: &str) -> Result<(), TaprootError> {
    if hash.len() != 64 {
        return Err(TaprootError::InvalidHash(format!(
            "hash must be 64 hex chars, got {} chars",
            hash.len()
        )));
    }
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(TaprootError::InvalidHash("hash must be hex".to_string()));
    }
    // Ensure lowercase for consistency (but accept any case on read).
    // Storage uses lowercase hex from StateEngine::hash.
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), TaprootError> {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::StateEngine;
    use crate::state::TaprootState;

    fn sample_state(repo: &str, branch: &str, commit: &str) -> TaprootState {
        TaprootState::new(repo, branch, commit)
            .with_runtime("python", "3.11.4")
            .with_env("FOO", "bar")
    }

    fn signed_sample(repo: &str, branch: &str) -> SignedState {
        let state = sample_state(repo, branch, "abc123");
        let (priv_key, _) = StateEngine::generate_keypair();
        StateEngine::sign(&state, &priv_key).unwrap()
    }

    #[test]
    fn init_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let reg_path = dir.path().join("registry");
        let reg = Registry::init(&reg_path).unwrap();
        assert!(reg.objects_dir().exists());
        assert!(reg.refs_dir().exists());
        // idempotent
        let reg2 = Registry::init(&reg_path).unwrap();
        assert_eq!(reg.root(), reg2.root());
    }

    #[test]
    fn push_and_pull_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("myapp", "main");
        let hash = reg.push(&signed).unwrap();
        assert_eq!(hash, signed.hash);
        let pulled = reg.pull(&hash).unwrap();
        assert_eq!(pulled, signed);
    }

    #[test]
    fn push_updates_ref_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("org/myapp", "main");
        reg.push(&signed).unwrap();
        let resolved = reg.resolve_ref("org/myapp", "main").unwrap();
        assert_eq!(resolved, Some(signed.hash.clone()));
    }

    #[test]
    fn branch_with_slash_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("myapp", "feat/foo");
        reg.push(&signed).unwrap();
        // File should be feat%2Ffoo
        let ref_path = reg.refs_dir().join("myapp").join("feat%2Ffoo");
        assert!(ref_path.exists());
        let list = reg.list("myapp").unwrap();
        assert!(list
            .iter()
            .any(|(b, h)| b == "feat/foo" && h == &signed.hash));
        assert_eq!(
            reg.resolve_ref("myapp", "feat/foo").unwrap(),
            Some(signed.hash)
        );
    }

    #[test]
    fn repo_with_slash_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("org/name", "main");
        reg.push(&signed).unwrap();
        let ref_path = reg.refs_dir().join("org%2Fname").join("main");
        assert!(ref_path.exists());
        let list = reg.list("org/name").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "main");
    }

    #[test]
    fn pull_missing_hash_errors() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let fake = "a".repeat(64);
        let err = reg.pull(&fake).unwrap_err();
        assert!(matches!(err, TaprootError::ObjectNotFound(_)));
    }

    #[test]
    fn pull_validates_hash_format() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let err = reg.pull("not-hex").unwrap_err();
        assert!(matches!(err, TaprootError::InvalidHash(_)));
    }

    #[test]
    fn resolve_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        assert_eq!(reg.resolve_ref("nope", "main").unwrap(), None);
    }

    #[test]
    fn list_empty_repo() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        assert!(reg.list("empty").unwrap().is_empty());
    }

    #[test]
    fn list_sorted_and_multiple_branches() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let s1 = signed_sample("myapp", "zebra");
        let s2 = signed_sample("myapp", "alpha");
        let s3 = signed_sample("myapp", "main");
        reg.push(&s1).unwrap();
        reg.push(&s2).unwrap();
        reg.push(&s3).unwrap();
        let list = reg.list("myapp").unwrap();
        let branches: Vec<_> = list.iter().map(|(b, _)| b.as_str()).collect();
        let mut sorted = branches.clone();
        sorted.sort();
        assert_eq!(branches, sorted);
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn log_returns_single_entry() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("myapp", "main");
        reg.push(&signed).unwrap();
        let log = reg.log("myapp", "main").unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].hash, signed.hash);
        assert!(reg.log("myapp", "missing").unwrap().is_empty());
    }

    #[test]
    fn push_verifies_hash_and_signature() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let mut signed = signed_sample("myapp", "main");
        // Tamper state without updating hash
        signed.state.env_vars.insert("EVIL".into(), "1".into());
        let err = reg.push(&signed).unwrap_err();
        assert!(matches!(
            err,
            TaprootError::HashMismatch { .. } | TaprootError::InvalidSignature
        ));
    }

    #[test]
    fn push_rejects_invalid_repo_branch() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let mut state = sample_state("myapp", "main", "abc123");
        state.base.repo = "".into();
        let (priv_key, _) = StateEngine::generate_keypair();
        let mut signed = StateEngine::sign(&state, &priv_key).unwrap();
        // Manually set empty repo after sign? Sign will have computed hash; push should reject via validate
        signed.state.base.repo = "".into();
        // Need to re-hash to pass hash check but fail repo validation — easiest: push with empty repo directly
        let err = reg.push(&signed).unwrap_err();
        assert!(matches!(
            err,
            TaprootError::InvalidKey(_) | TaprootError::HashMismatch { .. }
        ));
    }

    #[test]
    fn pull_detects_tampered_object() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("myapp", "main");
        let hash = reg.push(&signed).unwrap();
        // Tamper file on disk
        let obj_path = reg.object_path(&hash);
        let mut tampered: SignedState = signed.clone();
        tampered.state.env_vars.insert("TAMPER".into(), "1".into());
        let bytes = serde_json::to_vec_pretty(&tampered).unwrap();
        fs::write(&obj_path, bytes).unwrap();
        let err = reg.pull(&hash).unwrap_err();
        assert!(matches!(
            err,
            TaprootError::HashMismatch { .. } | TaprootError::InvalidSignature
        ));
    }

    #[test]
    fn validate_hash_rejects_bad() {
        assert!(validate_hash("abc").is_err());
        assert!(validate_hash(&"g".repeat(64)).is_err());
        assert!(validate_hash(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn sanitize_roundtrip() {
        assert_eq!(sanitize("org/myapp"), "org%2Fmyapp");
        assert_eq!(sanitize("feat/foo/bar"), "feat%2Ffoo%2Fbar");
        assert_eq!(desanitize("feat%2Ffoo"), "feat/foo");
        assert_eq!(desanitize(&sanitize("a/b/c")), "a/b/c");
        // collision test: a/b vs a__b must not collide
        assert_ne!(sanitize("a/b"), sanitize("a__b"));
        // percent escaping
        assert_eq!(sanitize("a%b"), "a%25b");
        assert_eq!(desanitize(&sanitize("a%b/c")), "a%b/c");
    }

    #[test]
    fn push_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("myapp", "main");
        let h1 = reg.push(&signed).unwrap();
        let h2 = reg.push(&signed).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(reg.list("myapp").unwrap().len(), 1);
    }

    #[test]
    fn unsigned_push_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let state = sample_state("myapp", "main", "abc123");
        let hash = StateEngine::hash(&state).unwrap();
        let signed = SignedState {
            state,
            hash: hash.clone(),
            signature: None,
            public_key: None,
        };
        let h = reg.push(&signed).unwrap();
        assert_eq!(h, hash);
        let pulled = reg.pull(&hash).unwrap();
        assert_eq!(pulled.signature, None);
    }
}
