use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crate::error::TaprootError;
use crate::state::{SignedState, TaprootState};

pub struct StateEngine;

impl StateEngine {
    /// Canonical JSON — sorted keys, no whitespace tricks.
    /// Uses serde_json with BTreeMap already sorted, then re-serializes deterministically.
    pub fn to_canonical_json(state: &TaprootState) -> Result<Vec<u8>, TaprootError> {
        // serde_json sorts struct keys by definition order; BTreeMap sorts env_vars.
        // For true canonical, we serialize via Value then to_string with sorted keys.
        let json = serde_json::to_string(state)?;
        // Parse and re-stringify to ensure deterministic key ordering at all levels.
        // serde_json's Value uses BTreeMap internally when `preserve_order` is off (default).
        let value: serde_json::Value = serde_json::from_str(&json)?;
        Ok(serde_json::to_vec(&value)?)
    }

    pub fn serialize(state: &TaprootState) -> Result<Vec<u8>, TaprootError> {
        Ok(serde_json::to_vec_pretty(state)?)
    }

    pub fn deserialize(bytes: &[u8]) -> Result<TaprootState, TaprootError> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// sha256 hex of canonical JSON
    pub fn hash(state: &TaprootState) -> Result<String, TaprootError> {
        let canonical = Self::to_canonical_json(state)?;
        let mut hasher = Sha256::new();
        hasher.update(&canonical);
        Ok(hex::encode(hasher.finalize()))
    }

    /// Generate a fresh ed25519 keypair. Returns (private_key_b64, public_key_b64)
    pub fn generate_keypair() -> (String, String) {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        (
            B64.encode(signing_key.to_bytes()),
            B64.encode(verifying_key.to_bytes()),
        )
    }

    /// Sign state. Returns SignedState with hash + signature.
    pub fn sign(state: &TaprootState, private_key_b64: &str) -> Result<SignedState, TaprootError> {
        let hash_hex = Self::hash(state)?;
        let hash_bytes = hex::decode(&hash_hex)
            .map_err(|e| TaprootError::InvalidKey(format!("hash not hex: {e}")))?;

        let key_bytes = B64
            .decode(private_key_b64.trim())
            .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
        let key_arr: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| TaprootError::InvalidKey("private key must be 32 bytes".into()))?;
        let signing_key = SigningKey::from_bytes(&key_arr);
        let signature: Signature = signing_key.sign(&hash_bytes);

        let public_key_b64 = B64.encode(signing_key.verifying_key().to_bytes());

        Ok(SignedState {
            state: state.clone(),
            hash: hash_hex,
            signature: Some(B64.encode(signature.to_bytes())),
            public_key: Some(public_key_b64),
        })
    }

    /// Verify SignedState. Checks hash matches state and signature is valid.
    pub fn verify(signed: &SignedState) -> Result<(), TaprootError> {
        let computed = Self::hash(&signed.state)?;
        if computed != signed.hash {
            return Err(TaprootError::HashMismatch {
                expected: signed.hash.clone(),
                got: computed,
            });
        }

        match (&signed.signature, &signed.public_key) {
            (Some(sig_b64), Some(pub_b64)) => {
                let sig_bytes = B64
                    .decode(sig_b64)
                    .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
                let pub_bytes = B64
                    .decode(pub_b64)
                    .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;

                let sig_arr: [u8; 64] = sig_bytes
                    .try_into()
                    .map_err(|_| TaprootError::InvalidKey("signature must be 64 bytes".into()))?;
                let pub_arr: [u8; 32] = pub_bytes
                    .try_into()
                    .map_err(|_| TaprootError::InvalidKey("public key must be 32 bytes".into()))?;

                let verifying_key = VerifyingKey::from_bytes(&pub_arr)
                    .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
                let signature = Signature::from_bytes(&sig_arr);

                let hash_bytes = hex::decode(&signed.hash)
                    .map_err(|e| TaprootError::InvalidKey(format!("hash not hex: {e}")))?;
                verifying_key
                    .verify(&hash_bytes, &signature)
                    .map_err(|_| TaprootError::InvalidSignature)
            }
            (None, None) => Ok(()), // unsigned is okay, hash already checked
            _ => Err(TaprootError::InvalidKey(
                "signature and public_key must both be present or both absent".into(),
            )),
        }
    }

    /// Save signed state to file (pretty JSON) — atomic via tempfile in same dir
    pub fn save(path: &std::path::Path, signed: &SignedState) -> Result<(), TaprootError> {
        use std::io::Write;
        let bytes = serde_json::to_vec_pretty(signed)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        if !parent.as_os_str().is_empty() && parent != std::path::Path::new(".") {
            std::fs::create_dir_all(parent)?;
        }
        // Use tempfile with random suffix to avoid symlink races and collisions
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(&bytes)?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| TaprootError::Io(e.error))?;
        // fsync parent dir for durability
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// Load signed state from file and verify
    pub fn load(path: &std::path::Path) -> Result<SignedState, TaprootError> {
        let bytes = std::fs::read(path)?;
        let signed: SignedState = serde_json::from_slice(&bytes)?;
        Self::verify(&signed)?;
        Ok(signed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> TaprootState {
        TaprootState::new("myapp", "main", "9f3a2c1")
            .with_runtime("python", "3.11.4")
            .with_runtime("node", "20.5.0")
            .with_container("postgres", "15.3", "postgres:15.3")
            .with_env("DATABASE_URL", "postgres://localhost/taproot")
            .with_env("NODE_ENV", "development")
    }

    #[test]
    fn roundtrip_json() {
        let state = sample_state();
        let bytes = StateEngine::serialize(&state).unwrap();
        let decoded = StateEngine::deserialize(&bytes).unwrap();
        assert_eq!(state.base, decoded.base);
        assert_eq!(state.runtimes, decoded.runtimes);
    }

    #[test]
    fn hash_is_deterministic() {
        let state = sample_state();
        let h1 = StateEngine::hash(&state).unwrap();
        let h2 = StateEngine::hash(&state).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // hex sha256
    }

    #[test]
    fn hash_changes_on_mutation() {
        let s1 = sample_state();
        let mut s2 = s1.clone();
        s2.env_vars.insert("NEW".into(), "1".into());
        assert_ne!(
            StateEngine::hash(&s1).unwrap(),
            StateEngine::hash(&s2).unwrap()
        );
    }

    #[test]
    fn sign_and_verify() {
        let state = sample_state();
        let (priv_b64, _) = StateEngine::generate_keypair();
        let signed = StateEngine::sign(&state, &priv_b64).unwrap();
        assert!(StateEngine::verify(&signed).is_ok());
    }

    #[test]
    fn verify_fails_on_tamper() {
        let state = sample_state();
        let (priv_b64, _) = StateEngine::generate_keypair();
        let mut signed = StateEngine::sign(&state, &priv_b64).unwrap();
        signed.state.env_vars.insert("EVIL".into(), "1".into());
        assert!(StateEngine::verify(&signed).is_err());
    }

    #[test]
    fn verify_fails_on_wrong_key() {
        let state = sample_state();
        let (priv_b64, _) = StateEngine::generate_keypair();
        let (other_priv, _) = StateEngine::generate_keypair();
        let mut signed = StateEngine::sign(&state, &priv_b64).unwrap();
        // re-sign hash with wrong key but keep hash
        let other_signed = StateEngine::sign(&state, &other_priv).unwrap();
        signed.signature = other_signed.signature;
        signed.public_key = other_signed.public_key;
        // Now tamper one more way: sign with other key but verify should use that key — it will pass.
        // So instead test: keep original signature, swap pubkey
        let mut tampered = StateEngine::sign(&state, &priv_b64).unwrap();
        let (_, other_pub) = StateEngine::generate_keypair();
        tampered.public_key = Some(other_pub);
        assert!(StateEngine::verify(&tampered).is_err());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let state = sample_state();
        let (priv_b64, _) = StateEngine::generate_keypair();
        let signed = StateEngine::sign(&state, &priv_b64).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        StateEngine::save(&path, &signed).unwrap();
        let loaded = StateEngine::load(&path).unwrap();
        assert_eq!(signed.hash, loaded.hash);
    }
}
