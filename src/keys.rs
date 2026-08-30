use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};

use crate::error::TaprootError;

/// A stored keypair — private stays local, public is shared.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keypair {
    pub id: String,
    /// base64 32 bytes
    pub private_key: String,
    /// base64 32 bytes
    pub public_key: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub active: bool,
}

/// Keystore on disk: `.taproot/keys/` with `keys.json` index + per-key files.
pub struct KeyStore {
    root: PathBuf,
}

impl KeyStore {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn init(root: &Path) -> Result<Self, TaprootError> {
        fs::create_dir_all(root)?;
        let ks = Self::new(root);
        // ensure index exists
        let idx = ks.index_path();
        if !idx.exists() {
            fs::write(&idx, b"[]")?;
        }
        Ok(ks)
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("keys.json")
    }

    fn key_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.key"))
    }

    /// Generate a fresh keypair, persist, return it.
    pub fn generate(&self, id: Option<String>) -> Result<Keypair, TaprootError> {
        let (priv_b64, pub_b64) = crate::engine::StateEngine::generate_keypair();
        let id = id.unwrap_or_else(|| {
            let pub_bytes = B64.decode(&pub_b64).unwrap_or_default();
            let hex_prefix = hex::encode(&pub_bytes[..4.min(pub_bytes.len())]);
            format!("key-{hex_prefix}")
        });
        // validate id
        if id.contains('/') || id.contains('\\') || id.contains('\0') || id.trim().is_empty() {
            return Err(TaprootError::InvalidKey("invalid key id".into()));
        }
        if self.key_path(&id).exists() {
            return Err(TaprootError::InvalidKey(format!("key id already exists: {id}")));
        }
        // verify base64 decodes to 32 bytes
        let priv_bytes = B64.decode(&priv_b64).map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
        if priv_bytes.len() != 32 {
            return Err(TaprootError::InvalidKey("private key must be 32 bytes".into()));
        }

        let kp = Keypair {
            id: id.clone(),
            private_key: priv_b64,
            public_key: pub_b64,
            created_at: chrono::Utc::now(),
            active: true,
        };

        // write key file (private) with 0600
        let key_json = serde_json::to_string_pretty(&kp)?;
        let path = self.key_path(&id);
        atomic_write(&path, key_json.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }

        // update index
        self.add_to_index(&kp)?;
        tracing::info!(id=%kp.id, "key generated");
        Ok(kp)
    }

    fn load_index(&self) -> Result<Vec<Keypair>, TaprootError> {
        let idx = self.index_path();
        if !idx.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&idx)?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn save_index(&self, keys: &[Keypair]) -> Result<(), TaprootError> {
        let bytes = serde_json::to_vec_pretty(keys)?;
        atomic_write(&self.index_path(), &bytes)?;
        Ok(())
    }

    fn add_to_index(&self, kp: &Keypair) -> Result<(), TaprootError> {
        let mut keys = self.load_index()?;
        // deactivate previous active keys if this one is active? keep all active for rotation
        keys.push(kp.clone());
        self.save_index(&keys)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Keypair>, TaprootError> {
        self.load_index()
    }

    pub fn get(&self, id: &str) -> Result<Keypair, TaprootError> {
        let path = self.key_path(id);
        if !path.exists() {
            return Err(TaprootError::InvalidKey(format!("key not found: {id}")));
        }
        let bytes = fs::read(&path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Get the most recent active key, or any key if none active.
    pub fn default_key(&self) -> Result<Keypair, TaprootError> {
        let keys = self.load_index()?;
        if keys.is_empty() {
            return Err(TaprootError::InvalidKey(
                "no keys found — run `taproot keys generate`".into(),
            ));
        }
        // prefer active, most recent
        if let Some(kp) = keys.iter().rev().find(|k| k.active) {
            return Ok(kp.clone());
        }
        Ok(keys.last().unwrap().clone())
    }

    /// Load private key b64 by id or default.
    pub fn private_key(&self, id: Option<&str>) -> Result<String, TaprootError> {
        let kp = match id {
            Some(i) => self.get(i)?,
            None => self.default_key()?,
        };
        Ok(kp.private_key)
    }

    /// Export public keys map id -> pubkey b64
    pub fn public_keys(&self) -> Result<BTreeMap<String, String>, TaprootError> {
        let keys = self.load_index()?;
        Ok(keys.into_iter().map(|k| (k.id, k.public_key)).collect())
    }

    /// Rotate: generate new active key, mark old ones inactive if requested.
    pub fn rotate(&self, deactivate_old: bool) -> Result<Keypair, TaprootError> {
        let kp = self.generate(None)?;
        if deactivate_old {
            let mut keys = self.load_index()?;
            for k in &mut keys {
                if k.id != kp.id {
                    k.active = false;
                }
            }
            // rewrite key files to reflect inactive
            for k in &keys {
                if k.id != kp.id {
                    let path = self.key_path(&k.id);
                    if path.exists() {
                        let json = serde_json::to_string_pretty(k)?;
                        atomic_write(&path, json.as_bytes())?;
                    }
                }
            }
            self.save_index(&keys)?;
        }
        Ok(kp)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), TaprootError> {
    use std::io::Write;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let ks = KeyStore::init(&dir.path().join("keys")).unwrap();
        let kp = ks.generate(Some("test-key".into())).unwrap();
        assert_eq!(kp.id, "test-key");
        assert_eq!(kp.private_key.len(), 44); // base64 32 bytes
        let loaded = ks.get("test-key").unwrap();
        assert_eq!(loaded.public_key, kp.public_key);
        let def = ks.default_key().unwrap();
        assert_eq!(def.id, "test-key");
    }

    #[test]
    fn list_and_default() {
        let dir = tempfile::tempdir().unwrap();
        let ks = KeyStore::init(&dir.path().join("keys")).unwrap();
        ks.generate(Some("k1".into())).unwrap();
        ks.generate(Some("k2".into())).unwrap();
        let list = ks.list().unwrap();
        assert_eq!(list.len(), 2);
        let def = ks.default_key().unwrap();
        assert_eq!(def.id, "k2");
    }

    #[test]
    fn rotate_deactivates_old() {
        let dir = tempfile::tempdir().unwrap();
        let ks = KeyStore::init(&dir.path().join("keys")).unwrap();
        ks.generate(Some("old".into())).unwrap();
        let new = ks.rotate(true).unwrap();
        assert!(new.active);
        let old = ks.get("old").unwrap();
        assert!(!old.active);
    }

    #[test]
    fn private_key_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let ks = KeyStore::init(&dir.path().join("keys")).unwrap();
        ks.generate(Some("mykey".into())).unwrap();
        let pk = ks.private_key(Some("mykey")).unwrap();
        assert!(!pk.is_empty());
        let def_pk = ks.private_key(None).unwrap();
        assert_eq!(pk, def_pk);
    }
}
