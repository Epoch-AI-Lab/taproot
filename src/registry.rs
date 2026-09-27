use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::StateEngine;
use crate::error::TaprootError;
use crate::state::SignedState;
use crate::util::{atomic_write, validate_non_empty};

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

/// Upper bound on `log` chain walks, so a corrupted or cyclic parent link
/// cannot spin forever. Far above any real branch history.
const MAX_LOG_HOPS: usize = 10_000;

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
    ///
    /// The ref's current hash is recorded as the new object's `parent`, so
    /// `log` can walk a branch back to its first push. A re-push of the same
    /// hash is a no-op for history, so pushing twice does not fork the chain.
    ///
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

        // Link to whatever this ref pointed at before, but not to itself: a
        // re-push of the same object must not make the chain cyclic.
        let ref_path = self.ref_path(&signed.state.base.repo, &signed.state.base.branch)?;
        let previous = self.resolve_ref(&signed.state.base.repo, &signed.state.base.branch)?;
        let parent = previous.filter(|p| p != &signed.hash);

        let mut stored = signed.clone();
        if parent.is_some() {
            stored.parent = parent;
        }

        // Write object atomically if not already present.
        let obj_path = self.object_path(&stored.hash);
        if !obj_path.exists() {
            let bytes = serde_json::to_vec_pretty(&stored)?;
            atomic_write(&obj_path, &bytes)?;
            tracing::info!(hash=%stored.hash, ?obj_path, "registry object written");
        } else {
            // Verify existing object matches (defensive).
            let existing = self.pull(&stored.hash)?;
            if existing.state != stored.state {
                tracing::warn!(hash=%stored.hash, "registry object exists with different content");
            }
        }

        // Update ref atomically.
        if let Some(parent) = ref_path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&ref_path, stored.hash.as_bytes())?;
        tracing::info!(
            repo=%stored.state.base.repo,
            branch=%stored.state.base.branch,
            hash=%stored.hash,
            "registry ref updated"
        );

        Ok(stored.hash.clone())
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

    /// Log for a repo/branch. Returns every state ever pushed to that ref,
    /// newest first, following the parent chain recorded at push time.
    pub fn log(&self, repo: &str, branch: &str) -> Result<Vec<SignedState>, TaprootError> {
        let Some(head) = self.resolve_ref(repo, branch)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut cursor = Some(head);
        // Bounded so a corrupted or cyclic parent chain cannot spin forever.
        let mut hops = 0usize;
        while let Some(hash) = cursor {
            if hops > MAX_LOG_HOPS {
                tracing::warn!(repo, branch, "log chain longer than limit, truncating");
                break;
            }
            hops += 1;
            let signed = self.pull(&hash)?;
            cursor = signed.parent.clone();
            out.push(signed);
        }
        Ok(out)
    }

    /// Parent hashes recorded for a ref, newest first. `None` when the ref has
    /// never been pushed to.
    pub fn history(&self, repo: &str, branch: &str) -> Result<Vec<String>, TaprootError> {
        Ok(self
            .log(repo, branch)?
            .into_iter()
            .map(|s| s.hash)
            .collect())
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

    /// Sign a state with a throwaway key, for tests that need several
    /// distinct objects in one registry.
    fn sign(state: &TaprootState) -> SignedState {
        let (priv_key, _) = StateEngine::generate_keypair();
        StateEngine::sign(state, &priv_key).unwrap()
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
    fn log_walks_parent_chain_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let first = signed_sample("myapp", "main");
        reg.push(&first).unwrap();

        let mut second = sample_state("myapp", "main", "def456");
        second.env_vars.insert("SECOND".into(), "1".into());
        let second = sign(&second);
        reg.push(&second).unwrap();

        let mut third = sample_state("myapp", "main", "aaa111");
        third.env_vars.insert("THIRD".into(), "1".into());
        let third = sign(&third);
        reg.push(&third).unwrap();

        let log = reg.log("myapp", "main").unwrap();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].hash, third.hash, "newest first");
        assert_eq!(log[1].hash, second.hash);
        assert_eq!(log[2].hash, first.hash);
        // The oldest entry has no parent.
        assert_eq!(log[2].parent, None);
        assert_eq!(log[0].parent.as_deref(), Some(second.hash.as_str()));
    }

    #[test]
    fn repush_same_hash_does_not_extend_history() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let signed = signed_sample("myapp", "main");
        reg.push(&signed).unwrap();
        reg.push(&signed).unwrap();
        reg.push(&signed).unwrap();
        // Self-parenting would make the walk loop forever.
        assert_eq!(reg.log("myapp", "main").unwrap().len(), 1);
    }

    #[test]
    fn history_is_hashes_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        let first = signed_sample("myapp", "main");
        reg.push(&first).unwrap();
        let second = sign(&sample_state("myapp", "main", "zzz999"));
        reg.push(&second).unwrap();
        assert_eq!(
            reg.history("myapp", "main").unwrap(),
            vec![second.hash, first.hash]
        );
    }

    #[test]
    fn log_on_unknown_branch_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::init(&dir.path().join("reg")).unwrap();
        reg.push(&signed_sample("myapp", "main")).unwrap();
        assert!(reg.log("myapp", "never-pushed").unwrap().is_empty());
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
            parent: None,
        };
        let h = reg.push(&signed).unwrap();
        assert_eq!(h, hash);
        let pulled = reg.pull(&hash).unwrap();
        assert_eq!(pulled.signature, None);
    }
}
