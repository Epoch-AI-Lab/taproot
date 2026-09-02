use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Git baseline this environment inherits from.
/// Think `main@9f3a2c1` — branch + commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseRef {
    pub repo: String,
    pub branch: String,
    pub commit: String,
}

/// A pinned runtime — python 3.11.4, node 20.5.0, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runtime {
    pub name: String,
    pub version: String,
    #[serde(default = "default_true")]
    pub pinned: bool,
}

fn default_true() -> bool {
    true
}

/// A containerized service — postgres 15.3 etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Container {
    pub name: String,
    pub version: String,
    /// e.g. postgres:15.3 or full digest
    pub image: String,
    #[serde(default)]
    pub signed: bool,
}

/// The core state object — everything needed to reproduce the env.
/// BTreeMap for env_vars ensures deterministic ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaprootState {
    /// Schema version, e.g. "1.0"
    pub version: String,
    pub base: BaseRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtimes: Vec<Runtime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub containers: Vec<Container>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_vars: BTreeMap<String, String>,
    /// When snapshot was taken
    pub created_at: DateTime<Utc>,
    /// Optional freeform notes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl TaprootState {
    pub fn new(
        repo: impl Into<String>,
        branch: impl Into<String>,
        commit: impl Into<String>,
    ) -> Self {
        Self {
            version: "1.0".to_string(),
            base: BaseRef {
                repo: repo.into(),
                branch: branch.into(),
                commit: commit.into(),
            },
            runtimes: Vec::new(),
            containers: Vec::new(),
            env_vars: BTreeMap::new(),
            created_at: Utc::now(),
            notes: None,
        }
    }

    pub fn with_runtime(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.runtimes.push(Runtime {
            name: name.into(),
            version: version.into(),
            pinned: true,
        });
        self
    }

    pub fn with_container(
        mut self,
        name: impl Into<String>,
        version: impl Into<String>,
        image: impl Into<String>,
    ) -> Self {
        self.containers.push(Container {
            name: name.into(),
            version: version.into(),
            image: image.into(),
            signed: true,
        });
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }
}

/// State + its integrity envelope. Hash is sha256 of canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedState {
    pub state: TaprootState,
    /// hex sha256, no prefix
    pub hash: String,
    /// base64 ed25519 signature over hash bytes, None if unsigned
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// base64 public key that signed it, if any
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}
