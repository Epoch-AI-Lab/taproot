use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::TaprootError;
use crate::util::atomic_write;

/// Audit log entry — append-only JSONL at `.taproot/registry/audit.log`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts: DateTime<Utc>,
    pub action: String, // "push", "pull", "verify"
    pub repo: String,
    pub branch: String,
    pub hash: String,
    pub actor: String, // token id or "local"
    pub signed: bool,
}

/// Policy for a repo: strict checks, required signing, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub repo: String,
    #[serde(default = "default_true")]
    pub require_signed: bool,
    #[serde(default = "default_true")]
    pub require_check_strict: bool,
    #[serde(default)]
    pub allowed_branches: Vec<String>, // empty = all
    #[serde(default)]
    pub blocked_env_keys: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            repo: "*".into(),
            require_signed: true,
            require_check_strict: true,
            allowed_branches: Vec::new(),
            blocked_env_keys: Vec::new(),
        }
    }
}

/// Fabric handles org-scoped registry with audit, policy, and tokens.
pub struct Fabric {
    root: PathBuf, // .taproot/fabric
    registry_root: PathBuf,
}

impl Fabric {
    pub fn new(fabric_root: &Path, registry_root: &Path) -> Self {
        Self {
            root: fabric_root.to_path_buf(),
            registry_root: registry_root.to_path_buf(),
        }
    }

    pub fn init(fabric_root: &Path, registry_root: &Path) -> Result<Self, TaprootError> {
        fs::create_dir_all(fabric_root)?;
        fs::create_dir_all(registry_root)?;
        let f = Self::new(fabric_root, registry_root);
        // ensure default files
        let audit = f.audit_path();
        if !audit.exists() {
            fs::write(&audit, b"")?;
        }
        let tokens = f.tokens_path();
        if !tokens.exists() {
            fs::write(&tokens, b"{}")?;
        }
        Ok(f)
    }

    fn audit_path(&self) -> PathBuf {
        // audit lives alongside registry for simplicity
        self.registry_root.join("audit.log")
    }

    fn tokens_path(&self) -> PathBuf {
        self.root.join("tokens.json")
    }

    fn policy_path(&self, repo: &str) -> PathBuf {
        let sanitized = crate::registry::sanitize(repo);
        self.root.join(format!("policy-{sanitized}.json"))
    }

    /// Append audit entry (atomic append).
    pub fn audit(&self, entry: AuditEntry) -> Result<(), TaprootError> {
        let path = self.audit_path();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        use std::io::Write;
        let line = serde_json::to_string(&entry)?;
        writeln!(file, "{line}")?;
        file.sync_all()?;
        tracing::info!(action=%entry.action, repo=%entry.repo, hash=%entry.hash, "audit");
        Ok(())
    }

    /// Read audit log, optionally filtered by repo.
    pub fn audit_log(&self, filter_repo: Option<&str>) -> Result<Vec<AuditEntry>, TaprootError> {
        let path = self.audit_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)?;
        let mut out = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(line)?;
            if let Some(r) = filter_repo {
                if entry.repo != r {
                    continue;
                }
            }
            out.push(entry);
        }
        Ok(out)
    }

    /// Token management — simple map token -> actor
    pub fn tokens(&self) -> Result<BTreeMap<String, String>, TaprootError> {
        let path = self.tokens_path();
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let bytes = fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(BTreeMap::new());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn add_token(&self, token: &str, actor: &str) -> Result<(), TaprootError> {
        if token.trim().is_empty() || token.contains('\n') || token.len() < 8 {
            return Err(TaprootError::InvalidKey("invalid token".into()));
        }
        let mut map = self.tokens()?;
        map.insert(token.to_string(), actor.to_string());
        let bytes = serde_json::to_vec_pretty(&map)?;
        atomic_write(&self.tokens_path(), &bytes)?;
        Ok(())
    }

    pub fn verify_token(&self, token: &str) -> Result<Option<String>, TaprootError> {
        let map = self.tokens()?;
        Ok(map.get(token).cloned())
    }

    /// Policy read/write
    pub fn get_policy(&self, repo: &str) -> Result<Policy, TaprootError> {
        let path = self.policy_path(repo);
        if !path.exists() {
            return Ok(Policy {
                repo: repo.to_string(),
                ..Default::default()
            });
        }
        let bytes = fs::read(&path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn set_policy(&self, policy: &Policy) -> Result<(), TaprootError> {
        let path = self.policy_path(&policy.repo);
        let bytes = serde_json::to_vec_pretty(policy)?;
        atomic_write(&path, &bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let fab = Fabric::init(&dir.path().join("fabric"), &dir.path().join("registry")).unwrap();
        let e = AuditEntry {
            ts: Utc::now(),
            action: "push".into(),
            repo: "myapp".into(),
            branch: "main".into(),
            hash: "a".repeat(64),
            actor: "tester".into(),
            signed: true,
        };
        fab.audit(e.clone()).unwrap();
        let log = fab.audit_log(Some("myapp")).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].hash, e.hash);
        assert!(fab.audit_log(Some("other")).unwrap().is_empty());
    }

    #[test]
    fn tokens_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let fab = Fabric::init(&dir.path().join("fabric"), &dir.path().join("registry")).unwrap();
        fab.add_token("secret-token-123", "alice").unwrap();
        assert_eq!(
            fab.verify_token("secret-token-123").unwrap(),
            Some("alice".into())
        );
        assert_eq!(fab.verify_token("bad").unwrap(), None);
    }

    #[test]
    fn policy_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let fab = Fabric::init(&dir.path().join("fabric"), &dir.path().join("registry")).unwrap();
        let p = Policy {
            repo: "myapp".into(),
            require_signed: true,
            require_check_strict: false,
            allowed_branches: vec!["main".into()],
            blocked_env_keys: vec!["SECRET".into()],
        };
        fab.set_policy(&p).unwrap();
        let loaded = fab.get_policy("myapp").unwrap();
        assert_eq!(loaded.require_check_strict, false);
        assert_eq!(loaded.allowed_branches, vec!["main"]);
        let def = fab.get_policy("other").unwrap();
        assert!(def.require_signed);
    }
}
