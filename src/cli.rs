use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use crate::engine::StateEngine;
use crate::error::TaprootError;
use crate::state::TaprootState;
use crate::util::validate_non_empty;

const DEFAULT_STATE_PATH: &str = ".taproot/state.json";

fn resolve_or_default(input: Option<PathBuf>, default: &str) -> PathBuf {
    input.unwrap_or_else(|| PathBuf::from(default))
}

fn resolve_state_path(input: Option<PathBuf>) -> PathBuf {
    resolve_or_default(input, DEFAULT_STATE_PATH)
}

fn display_state_path(path: &Path) -> String {
    // Show absolute if relative, to avoid cwd confusion noted in PR review
    if path.is_absolute() {
        path.display().to_string()
    } else if let Ok(cur) = std::env::current_dir() {
        cur.join(path).display().to_string()
    } else {
        path.display().to_string()
    }
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "taproot",
    version,
    about = "State inheritance fabric between VCS and CI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialise a new taproot state snapshot
    Init(InitArgs),
    /// Scan a project for declared runtimes, containers, and env vars
    Scan(ScanArgs),
    /// Mount a taproot state (env file writable; edits captured as drift)
    Mount(MountArgs),
    /// Show current state status
    Status(StatusArgs),
    /// Verify state signature and hash
    Verify(VerifyArgs),
    /// Review captured drift and re-sign it into the current state
    Sync(SyncArgs),
    /// Check current state against a baseline for drift (strict)
    Check(CheckArgs),
    /// Local signed state registry (content-addressed)
    Registry(RegistryArgs),
    /// Key management (ed25519)
    Keys(KeysArgs),
    /// Fabric: audit, policy, tokens
    Fabric(FabricArgs),
    /// Serve registry API (managed fabric)
    Serve(ServeArgs),
    /// Remote registry (push/pull via HTTP)
    Remote(RemoteArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Repository name (e.g. myapp or org/myapp)
    #[arg(long)]
    pub repo: String,

    /// Branch name (e.g. main or feat/foo)
    #[arg(long)]
    pub branch: String,

    /// Commit hash (e.g. 9f3a2c1)
    #[arg(long)]
    pub commit: String,

    /// Path to state file (default: .taproot/state.json, relative to current directory)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Skip signing (store hash only, no ed25519 signature)
    #[arg(long = "no-sign", default_value_t = false)]
    pub no_sign: bool,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Project directory to scan (default: current directory)
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Print the findings as JSON instead of a table
    #[arg(long, default_value_t = false)]
    pub json: bool,

    /// Write the findings into the state file and sign it
    #[arg(long, default_value_t = false)]
    pub apply: bool,

    /// Path to state file to write with --apply (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Include env vars from .env files (skipped by default, they often hold secrets)
    #[arg(long, default_value_t = false)]
    pub include_env: bool,
}

#[derive(Debug, Args)]
pub struct MountArgs {
    /// Path to mount (must be an existing empty directory). Not needed with
    /// --no-fuse, which writes the tree to --out instead.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Path to state file (default: .taproot/state.json, relative to current directory)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Write the mounted tree to a real directory instead of a FUSE mount (works without /dev/fuse)
    #[arg(long = "no-fuse", default_value_t = false)]
    pub no_fuse: bool,

    /// Where --no-fuse writes the tree (default: .taproot/mnt next to the state file)
    #[arg(long, value_name = "PATH")]
    pub out: Option<PathBuf>,

    /// Where to write captured drift (default: state.drift.json next to the state file)
    #[arg(long, value_name = "PATH")]
    pub drift_out: Option<PathBuf>,
}

/// Default directory for a materialized tree: `.taproot/mnt` beside the state.
fn default_materialize_dir(state_path: &Path) -> PathBuf {
    match state_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join("mnt"),
        _ => PathBuf::from("mnt"),
    }
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Path to baseline state file to re-sign into (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Path to drifted state to adopt (default: state.drift.json next to the state file)
    #[arg(long, value_name = "PATH")]
    pub from: Option<PathBuf>,

    /// Read drift from a materialized tree's env file (from `mount --no-fuse`)
    #[arg(long, value_name = "DIR", conflicts_with = "from")]
    pub from_dir: Option<PathBuf>,

    /// Show the diff report without adopting anything
    #[arg(long = "dry-run", default_value_t = false)]
    pub dry_run: bool,

    /// Adopt drift that touches non-env fields (base, runtimes, containers)
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Skip signing (store hash only, no ed25519 signature)
    #[arg(long = "no-sign", default_value_t = false)]
    pub no_sign: bool,

    /// Keep the drift file after a successful sync
    #[arg(long, default_value_t = false, conflicts_with = "dry_run")]
    pub keep: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Path to state file (default: .taproot/state.json, relative to current directory)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Path to state file (default: .taproot/state.json, relative to current directory)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Path to baseline state file to compare against
    #[arg(long, value_name = "PATH")]
    pub baseline: PathBuf,

    /// Path to current state file (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Machine-readable JSON output
    #[arg(long, default_value_t = false)]
    pub json: bool,

    /// Treat warnings as breaking (strict mode — default: true for CI, use --no-strict to disable)
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub strict: bool,

    /// Alias to disable strict mode
    #[arg(long = "no-strict", conflicts_with = "strict", hide = true)]
    pub no_strict: bool,

    /// Allow warnings without failing (overrides strict for warnings)
    #[arg(long, default_value_t = false)]
    pub allow_warnings: bool,
}

#[derive(Debug, Args)]
pub struct RegistryArgs {
    #[command(subcommand)]
    pub command: RegistryCommands,
}

#[derive(Debug, Subcommand)]
pub enum RegistryCommands {
    /// Push current state into the local registry (content-addressed + ref update)
    Push(RegistryPushArgs),
    /// Pull a state by hash from the registry
    Pull(RegistryPullArgs),
    /// List branches for a repo
    List(RegistryListArgs),
    /// Show a state by hash (alias for pull without writing)
    Show(RegistryShowArgs),
    /// Resolve a repo/branch ref to its hash
    Resolve(RegistryResolveArgs),
    /// Show log for a repo/branch (current ref)
    Log(RegistryLogArgs),
}

#[derive(Debug, Args)]
pub struct RegistryPushArgs {
    /// Path to state file (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,
    /// Registry root (default: .taproot/registry)
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RegistryPullArgs {
    /// Hash (64 hex) of the object to pull
    pub hash: String,
    /// Write pulled state to this file (default: stdout summary)
    #[arg(long, value_name = "PATH")]
    pub out: Option<PathBuf>,
    /// Registry root (default: .taproot/registry)
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RegistryListArgs {
    /// Repo name (e.g. myapp or org/myapp)
    pub repo: String,
    /// Registry root (default: .taproot/registry)
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RegistryShowArgs {
    /// Hash (64 hex) to show
    pub hash: String,
    /// Registry root (default: .taproot/registry)
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RegistryResolveArgs {
    /// Repo name
    pub repo: String,
    /// Branch name
    pub branch: String,
    /// Registry root (default: .taproot/registry)
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RegistryLogArgs {
    /// Repo name
    pub repo: String,
    /// Branch name
    pub branch: String,
    /// Registry root (default: .taproot/registry)
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

const DEFAULT_REGISTRY_PATH: &str = ".taproot/registry";
const DEFAULT_KEYS_PATH: &str = ".taproot/keys";
const DEFAULT_FABRIC_PATH: &str = ".taproot/fabric";

fn resolve_registry_path(input: Option<PathBuf>) -> PathBuf {
    resolve_or_default(input, DEFAULT_REGISTRY_PATH)
}
fn resolve_keys_path(input: Option<PathBuf>) -> PathBuf {
    resolve_or_default(input, DEFAULT_KEYS_PATH)
}
fn resolve_fabric_path(input: Option<PathBuf>) -> PathBuf {
    resolve_or_default(input, DEFAULT_FABRIC_PATH)
}

// ---------------------------------------------------------------------------
// Keys / Fabric / Serve / Remote CLI
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct KeysArgs {
    #[command(subcommand)]
    pub command: KeysCommands,
}

#[derive(Debug, Subcommand)]
pub enum KeysCommands {
    /// Generate a new ed25519 keypair
    Generate(KeysGenerateArgs),
    /// List stored keys
    List(KeysListArgs),
    /// Show a key by id
    Show(KeysShowArgs),
    /// Rotate keys (generate new active, optionally deactivate old)
    Rotate(KeysRotateArgs),
}

#[derive(Debug, Args)]
pub struct KeysGenerateArgs {
    /// Key id (default: key-<pubkey-prefix>)
    #[arg(long)]
    pub id: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub keys: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct KeysListArgs {
    #[arg(long, value_name = "PATH")]
    pub keys: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct KeysShowArgs {
    pub id: String,
    #[arg(long, value_name = "PATH")]
    pub keys: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct KeysRotateArgs {
    #[arg(long, default_value_t = false)]
    pub deactivate_old: bool,
    #[arg(long, value_name = "PATH")]
    pub keys: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct FabricArgs {
    #[command(subcommand)]
    pub command: FabricCommands,
}

#[derive(Debug, Subcommand)]
pub enum FabricCommands {
    /// Show audit log
    Audit(FabricAuditArgs),
    /// Get policy for a repo
    PolicyGet(FabricPolicyGetArgs),
    /// Set policy for a repo
    PolicySet(FabricPolicySetArgs),
    /// Add a bearer token (actor)
    TokenAdd(FabricTokenAddArgs),
    /// List tokens
    TokenList(FabricTokenListArgs),
}

#[derive(Debug, Args)]
pub struct FabricAuditArgs {
    /// Filter by repo (optional)
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub fabric: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct FabricPolicyGetArgs {
    pub repo: String,
    #[arg(long, value_name = "PATH")]
    pub fabric: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct FabricPolicySetArgs {
    pub repo: String,
    #[arg(long)]
    pub require_signed: Option<bool>,
    #[arg(long)]
    pub require_check_strict: Option<bool>,
    #[arg(long)]
    pub allow_branch: Vec<String>,
    #[arg(long)]
    pub block_env: Vec<String>,
    #[arg(long, value_name = "PATH")]
    pub fabric: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct FabricTokenAddArgs {
    pub token: String,
    pub actor: String,
    #[arg(long, value_name = "PATH")]
    pub fabric: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct FabricTokenListArgs {
    #[arg(long, value_name = "PATH")]
    pub fabric: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Bind address (default: 127.0.0.1:3000)
    #[arg(long, default_value = "127.0.0.1:3000")]
    pub addr: String,
    #[arg(long, value_name = "PATH")]
    pub registry: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub fabric: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommands,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommands {
    /// Push local state to remote registry
    Push(RemotePushArgs),
    /// Pull state from remote by hash
    Pull(RemotePullArgs),
    /// Resolve ref via remote
    Resolve(RemoteResolveArgs),
    /// Check drift via remote
    Check(RemoteCheckArgs),
}

#[derive(Debug, Args)]
pub struct RemotePushArgs {
    #[arg(long, value_name = "URL")]
    pub remote: String,
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,
    #[arg(long)]
    pub token: Option<String>,
}

#[derive(Debug, Args)]
pub struct RemotePullArgs {
    pub hash: String,
    #[arg(long, value_name = "URL")]
    pub remote: String,
    #[arg(long, value_name = "PATH")]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub token: Option<String>,
}

#[derive(Debug, Args)]
pub struct RemoteResolveArgs {
    pub repo: String,
    pub branch: String,
    #[arg(long, value_name = "URL")]
    pub remote: String,
    #[arg(long)]
    pub token: Option<String>,
}

#[derive(Debug, Args)]
pub struct RemoteCheckArgs {
    pub baseline_hash: String,
    pub current_hash: String,
    #[arg(long, value_name = "URL")]
    pub remote: String,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub strict: bool,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// Helpers — printing
// ---------------------------------------------------------------------------

fn print_mount_header(signed: &crate::state::SignedState) {
    let state = &signed.state;
    let short_hash = if signed.hash.len() >= 12 {
        &signed.hash[..12]
    } else {
        &signed.hash
    };
    let sig_label = if signed.signature.is_some() {
        "signed"
    } else {
        "unsigned"
    };

    println!("TAPROOT MOUNT");
    println!("─────────────────────────────────────────");
    println!("repo:       {}", state.base.repo);
    println!("base:       {}@{}", state.base.branch, state.base.commit);
    println!("state:      {sig_label} · sha256:{short_hash}");
    println!("runtimes:   {}", state.runtimes.len());
    for r in &state.runtimes {
        println!("  - {}: {} (pinned={})", r.name, r.version, r.pinned);
    }
    println!("containers: {}", state.containers.len());
    for c in &state.containers {
        println!("  - {}: {} ({})", c.name, c.version, c.image);
    }
    println!("env-vars:   {}", state.env_vars.len());
}

fn print_status_line(ok: bool) {
    if ok {
        println!("status:     ▶ INHERITED — ready to work");
    } else {
        println!("status:     ✗ DRIFTED — state verification failed");
    }
}

fn print_unsigned_warning() {
    println!("warning:    ⚠ UNSIGNED — hash ok, not cryptographically signed");
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub fn handle_scan(args: ScanArgs) -> Result<(), TaprootError> {
    let dir = args
        .dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    if !dir.is_dir() {
        return Err(TaprootError::Mount(format!(
            "not a directory: {}",
            dir.display()
        )));
    }

    let result = crate::scan::scan_project(&dir);
    let (env_vars, env_skipped) = if args.include_env {
        crate::scan::scan_env_vars(&dir)
    } else {
        (Default::default(), Default::default())
    };

    if args.json {
        let payload = serde_json::json!({
            "runtimes": result.runtimes,
            "containers": result.containers,
            "env_vars": env_vars,
            "env_skipped": env_skipped,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("TAPROOT SCAN");
        println!("─────────────────────────────────────────");
        println!("dir:        {}", dir.display());
        println!();

        if result.runtimes.is_empty() {
            println!("runtimes:   none detected");
            println!("            looked for .tool-versions, .mise.toml, Dockerfile, package.json");
        } else {
            println!("runtimes:   {}", result.runtimes.len());
            for r in &result.runtimes {
                println!("  {:<20} {} (pinned)", r.name, r.version);
            }
        }
        println!();

        if result.containers.is_empty() {
            println!("containers: none detected");
            println!("            looked for docker-compose.yml, compose.yml");
        } else {
            println!("containers: {}", result.containers.len());
            for c in &result.containers {
                println!("  {:<20} {} → {}", c.name, c.image, c.version);
            }
        }
        println!();

        if args.include_env {
            if env_vars.is_empty() {
                println!("env-vars:   none captured");
            } else {
                println!("env-vars:   {}", env_vars.len());
                for (k, v) in &env_vars {
                    println!("  {k}={v}");
                }
            }
            if !env_skipped.is_empty() {
                println!();
                println!(
                    "skipped {} value(s) that look like secrets:",
                    env_skipped.len()
                );
                for (k, why) in &env_skipped {
                    println!("  {k:<24} {why}");
                }
            }
        } else {
            println!("env-vars:   not read (pass --include-env to read .env files)");
        }
        println!();
    }

    if !args.apply {
        return Ok(());
    }

    // --apply writes the findings into the state file, creating it when absent
    // so `scan --apply` works before `init`.
    let state_path = resolve_state_path(args.state_path);
    let mut state = match StateEngine::load(&state_path) {
        Ok(signed) => signed.state,
        Err(_) => {
            // `file_name()` is None for "." and for a bare root path, so
            // canonicalize before naming the repo after the directory.
            let named = dir
                .canonicalize()
                .ok()
                .as_deref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let (branch, commit) = git_head(&dir);
            TaprootState::new(named, branch, commit)
        }
    };
    state = result.apply_to(state);
    if args.include_env {
        state.env_vars.extend(env_vars);
    }
    state.created_at = chrono::Utc::now();

    let keys_path = resolve_keys_path(None);
    let priv_key = if keys_path.exists() {
        crate::keys::KeyStore::init(&keys_path)
            .and_then(|ks| ks.default_key())
            .map(|kp| kp.private_key)
            .unwrap_or_else(|_| StateEngine::generate_keypair().0)
    } else {
        StateEngine::generate_keypair().0
    };
    let signed = StateEngine::sign(&state, &priv_key)?;
    if let Some(parent) = state_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    StateEngine::save(&state_path, &signed)?;
    println!(
        "applied:    sha256:{} → {}",
        &signed.hash[..12],
        display_state_path(&state_path)
    );
    Ok(())
}

/// Current branch and short commit for a directory, falling back to
/// placeholders when git is unavailable or the directory is not a repo.
fn git_head(dir: &Path) -> (String, String) {
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "main".into());
    let commit = run(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    (branch, commit)
}

pub fn handle_init(args: InitArgs) -> Result<(), TaprootError> {
    validate_non_empty("repo", &args.repo)?;
    validate_non_empty("branch", &args.branch)?;
    validate_non_empty("commit", &args.commit)?;

    let state_path = resolve_state_path(args.state_path);
    tracing::info!(?state_path, repo = %args.repo, "init state");

    let state = TaprootState::new(args.repo.clone(), args.branch.clone(), args.commit.clone());

    let signed = if args.no_sign {
        let hash = StateEngine::hash(&state)?;
        crate::state::SignedState {
            state,
            hash,
            signature: None,
            public_key: None,
            parent: None,
        }
    } else {
        // Prefer stored keys if available, else generate ephemeral
        let keys_path = resolve_keys_path(None);
        let (priv_key, key_info) = if keys_path.exists() {
            match crate::keys::KeyStore::init(&keys_path).and_then(|ks| {
                let kp = ks.default_key()?;
                Ok((
                    kp.private_key.clone(),
                    format!("key {} ({})", kp.id, &kp.public_key[..16]),
                ))
            }) {
                Ok((k, info)) => (k, Some(info)),
                Err(e) => {
                    println!(
                        "warning: could not load default key ({e}) — signing with ephemeral key"
                    );
                    (StateEngine::generate_keypair().0, None)
                }
            }
        } else {
            (StateEngine::generate_keypair().0, None)
        };
        let s = StateEngine::sign(&state, &priv_key)?;
        if let Some(info) = key_info {
            println!("signing with {info}");
        } else {
            println!("signing with ephemeral key (no keys found, run `taproot keys generate`)");
        }
        s
    };

    if let Some(parent) = state_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    StateEngine::save(&state_path, &signed)?;

    // Header — similar to README / mount but labelled for init
    let short_hash = if signed.hash.len() >= 12 {
        &signed.hash[..12]
    } else {
        &signed.hash
    };
    let sig_label = if signed.signature.is_some() {
        "signed"
    } else {
        "unsigned"
    };

    println!("TAPROOT INIT");
    println!("─────────────────────────────────────────");
    println!("repo:       {}", signed.state.base.repo);
    println!(
        "base:       {}@{}",
        signed.state.base.branch, signed.state.base.commit
    );
    println!("state:      {sig_label} · sha256:{short_hash}");
    println!("hash:       {}", signed.hash);
    if let Some(pk) = &signed.public_key {
        let preview = if pk.len() >= 16 { &pk[..16] } else { pk };
        println!("pubkey:     {preview}...");
    }
    println!("path:       {}", display_state_path(&state_path));
    println!();
    print_status_line(true);
    println!();
    println!("[next: taproot mount <path>]");

    Ok(())
}

fn default_drift_path(state_path: &Path) -> PathBuf {
    match state_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join("state.drift.json"),
        _ => PathBuf::from("state.drift.json"),
    }
}

/// Refuse when two paths resolve to the same file — e.g. `sync --from`
/// pointing at the baseline itself, or `--drift-out` overwriting it.
fn ensure_distinct(a: &Path, b: &Path, what: &str) -> Result<(), TaprootError> {
    // canonicalize resolves symlinks; fall back to cwd-relative absolutization
    // for paths that don't exist yet
    let resolve = |p: &Path| -> PathBuf {
        p.canonicalize().unwrap_or_else(|_| match p.is_absolute() {
            true => p.to_path_buf(),
            false => std::env::current_dir().unwrap_or_default().join(p),
        })
    };
    if resolve(a) == resolve(b) {
        return Err(TaprootError::InvalidPaths(format!(
            "{what} points at the baseline state file itself ({}): refusing",
            display_state_path(b)
        )));
    }
    Ok(())
}

/// Sign a state for adoption: stored default key if present, else ephemeral;
/// or hash-only with --no-sign.
pub fn sign_state_with_keys(
    state: TaprootState,
    no_sign: bool,
    keys_path: &Path,
) -> Result<crate::state::SignedState, TaprootError> {
    if no_sign {
        let hash = StateEngine::hash(&state)?;
        return Ok(crate::state::SignedState {
            state,
            hash,
            signature: None,
            public_key: None,
            parent: None,
        });
    }
    if keys_path.exists() {
        match crate::keys::KeyStore::init(keys_path) {
            Ok(ks) => match ks.default_key() {
                Ok(kp) => {
                    let preview = &kp.public_key[..16.min(kp.public_key.len())];
                    println!("signing with key {} ({preview})", kp.id);
                    return StateEngine::sign(&state, &kp.private_key);
                }
                Err(e) => println!(
                    "warning: could not load default key ({e}) — signing with ephemeral key"
                ),
            },
            Err(e) => {
                println!("warning: could not open keystore ({e}) — signing with ephemeral key")
            }
        }
    } else {
        println!("signing with ephemeral key (no keys found, run `taproot keys generate`)");
    }
    let (priv_key, _) = StateEngine::generate_keypair();
    StateEngine::sign(&state, &priv_key)
}

fn sign_for_adoption(
    state: TaprootState,
    no_sign: bool,
) -> Result<crate::state::SignedState, TaprootError> {
    sign_state_with_keys(state, no_sign, &resolve_keys_path(None))
}

pub fn handle_mount(args: MountArgs) -> Result<(), TaprootError> {
    let state_path = resolve_state_path(args.state_path);
    tracing::info!(?state_path, ?args.path, "mount");

    let signed = match StateEngine::load(&state_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "warning: failed to load state from {}: {e}",
                display_state_path(&state_path)
            );
            if !Path::new(&state_path).exists() {
                eprintln!("hint: run `taproot init --repo <repo> --branch <branch> --commit <commit>` first");
            }
            println!();
            println!("status:     ✗ ERROR — state not found or invalid");
            println!();
            return Err(e);
        }
    };

    print_mount_header(&signed);
    println!();

    // With --no-fuse there is no mountpoint: the tree goes to --out (or
    // .taproot/mnt), so all the mountpoint validation below is skipped.
    let target_meta = match &args.path {
        Some(p) => {
            println!("mount:      {}", p.display());
            let meta = std::fs::symlink_metadata(p);
            match &meta {
                Ok(m) if m.is_dir() => println!("target:     exists (directory)"),
                Ok(m) if m.file_type().is_symlink() => {
                    println!("target:     exists (symlink — will be rejected)")
                }
                Ok(_) => println!("target:     exists (not a directory — will be rejected)"),
                Err(_) => println!("target:     not found"),
            }
            Some(meta)
        }
        None => {
            println!("mount:      (none — materializing a tree)");
            None
        }
    };
    println!("hash:       {}", signed.hash);
    if signed.signature.is_none() {
        print_unsigned_warning();
    }
    println!();

    if args.path.is_none() && !args.no_fuse {
        let e = TaprootError::Mount("mountpoint is required unless --no-fuse is set".into());
        eprintln!("✗ mount failed: {e}");
        println!("status:     ✗ MOUNT FAILED — no mountpoint given");
        println!();
        return Err(e);
    }

    // Validate mountpoint before honoring --no-fuse — CI must not hide symlink/file attacks
    if let Some(Ok(m)) = target_meta.as_ref() {
        let path = args
            .path
            .as_ref()
            .expect("path present when metadata resolved");
        if m.file_type().is_symlink() {
            let e = TaprootError::Mount(format!(
                "mountpoint is a symlink (refusing): {}",
                path.display()
            ));
            eprintln!("✗ mount failed: {e}");
            println!("status:     ✗ MOUNT FAILED — symlink rejected");
            println!();
            return Err(e);
        }
        if !m.is_dir() {
            let e =
                TaprootError::Mount(format!("mountpoint is not a directory: {}", path.display()));
            eprintln!("✗ mount failed: {e}");
            println!("status:     ✗ MOUNT FAILED — not a directory");
            println!();
            return Err(e);
        }
    } else if !args.no_fuse {
        // real mount requires existing dir
        let shown = args
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into());
        let e = TaprootError::Mount(format!("mountpoint does not exist: {shown}"));
        eprintln!("✗ mount failed: {e}");
        println!("status:     ✗ MOUNT FAILED — mountpoint missing");
        println!();
        return Err(e);
    }

    if args.no_fuse {
        // Materialize the same tree a FUSE mount would serve, so the drift
        // loop is reachable where /dev/fuse is not available.
        let out = args
            .out
            .clone()
            .unwrap_or_else(|| default_materialize_dir(&state_path));
        ensure_distinct(&state_path, &out, "--out")?;
        match crate::mount::materialize_tree(&out, &signed) {
            Ok(()) => {
                println!("(no-fuse — wrote tree to {})", display_state_path(&out));
                println!(
                    "env:        {}/env (writable — edit, then run `taproot sync --from-dir`)",
                    display_state_path(&out)
                );
                let drift_path = args
                    .drift_out
                    .clone()
                    .unwrap_or_else(|| default_drift_path(&state_path));
                match crate::mount::capture_drift_from_dir(&out, &signed) {
                    Ok(Some(drift)) => {
                        StateEngine::save(&drift_path, &drift)?;
                        println!("drift:      captured — env differs from baseline");
                        println!("path:       {}", display_state_path(&drift_path));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("warn: could not read drift from {}: {e}", out.display());
                    }
                }
                print_status_line(true);
                println!();
                return Ok(());
            }
            Err(e) => {
                eprintln!("✗ mount failed: {e}");
                println!("status:     ✗ MOUNT FAILED — could not write tree");
                println!();
                return Err(e);
            }
        }
    }

    // Reached only on the real FUSE path, so a mountpoint is guaranteed here.
    let mountpoint = args.path.as_ref().ok_or_else(|| {
        TaprootError::Mount("mountpoint is required unless --no-fuse is set".into())
    })?;
    println!(
        "attempting FUSE mount at {} (env writable, Ctrl-C to unmount)...",
        mountpoint.display()
    );
    let drift_path = args
        .drift_out
        .clone()
        .unwrap_or_else(|| default_drift_path(&state_path));
    ensure_distinct(&state_path, &drift_path, "--drift-out")?;
    match crate::mount::mount_readonly(mountpoint, &signed) {
        Ok(outcome) => {
            print_status_line(true);
            if let Some(drift) = outcome.drift {
                if let Err(e) = StateEngine::save(&drift_path, &drift) {
                    eprintln!(
                        "✗ mounted OK, but failed to save drift to {}: {e}",
                        display_state_path(&drift_path)
                    );
                    return Err(e);
                }
                println!();
                println!("drift:      captured — env was edited during the mount");
                println!("path:       {}", display_state_path(&drift_path));
                println!("[next: taproot sync to review, sign, and adopt]");
            } else if let Some(raw) = outcome.raw_env {
                let raw_path = drift_path.with_extension("env.txt");
                let parse_msg = outcome
                    .parse_error
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unparseable env".to_string());
                match std::fs::write(&raw_path, &raw) {
                    Ok(()) => {
                        println!();
                        println!("drift:      ⚠ env was edited but could not be read: {parse_msg}");
                        println!(
                            "raw:        session preserved in {}",
                            display_state_path(&raw_path)
                        );
                        println!("[inspect the raw env file, fix or restore, then re-sign]");
                    }
                    Err(e) => {
                        eprintln!(
                            "✗ mounted OK, but failed to save raw env to {}: {e}",
                            display_state_path(&raw_path)
                        );
                        return Err(TaprootError::Io(e));
                    }
                }
            }
            println!();
            Ok(())
        }
        Err(e) => {
            eprintln!("✗ mount failed: {e}");
            println!("status:     ✗ MOUNT FAILED — {}", e);
            println!();
            Err(e)
        }
    }
}

pub fn handle_status(args: StatusArgs) -> Result<(), TaprootError> {
    let state_path = resolve_state_path(args.state_path);
    tracing::info!(?state_path, "status");

    let signed = StateEngine::load(&state_path)?;

    println!("TAPROOT STATUS");
    println!("─────────────────────────────────────────");
    // reuse same header but with correct title
    let short_hash = if signed.hash.len() >= 12 {
        &signed.hash[..12]
    } else {
        &signed.hash
    };
    let sig_label = if signed.signature.is_some() {
        "signed"
    } else {
        "unsigned"
    };
    println!("repo:       {}", signed.state.base.repo);
    println!(
        "base:       {}@{}",
        signed.state.base.branch, signed.state.base.commit
    );
    println!("state:      {sig_label} · sha256:{short_hash}");
    println!("runtimes:   {}", signed.state.runtimes.len());
    println!("containers: {}", signed.state.containers.len());
    println!("env-vars:   {}", signed.state.env_vars.len());
    println!();
    println!("hash:       {}", signed.hash);
    if let Some(pk) = &signed.public_key {
        let preview = if pk.len() >= 16 { &pk[..16] } else { pk };
        println!("pubkey:     {preview}...");
    }
    if signed.signature.is_none() {
        print_unsigned_warning();
    }
    println!("path:       {}", display_state_path(&state_path));
    println!();
    print_status_line(true);
    println!();

    Ok(())
}

pub fn handle_verify(args: VerifyArgs) -> Result<(), TaprootError> {
    let state_path = resolve_state_path(args.state_path);
    tracing::info!(?state_path, "verify");

    match StateEngine::load(&state_path) {
        Ok(signed) => {
            if signed.signature.is_none() {
                println!("⚠ verified (unsigned) — sha256:{}", signed.hash);
                println!(
                    "  repo: {}  base: {}@{}",
                    signed.state.base.repo, signed.state.base.branch, signed.state.base.commit
                );
                println!("  path: {}", display_state_path(&state_path));
                print_unsigned_warning();
            } else {
                println!("✓ verified — sha256:{}", signed.hash);
                println!(
                    "  repo: {}  base: {}@{}",
                    signed.state.base.repo, signed.state.base.branch, signed.state.base.commit
                );
                println!("  path: {}", display_state_path(&state_path));
            }
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "✗ verification failed for {}: {e}",
                display_state_path(&state_path)
            );
            Err(e)
        }
    }
}

pub fn handle_sync(args: SyncArgs) -> Result<(), TaprootError> {
    let state_path = resolve_state_path(args.state_path.clone());
    let baseline = StateEngine::load(&state_path)?;

    // `--from-dir` reads a materialized tree's env file directly, so the drift
    // loop works without a FUSE unmount having written a drift file first.
    if let Some(dir) = args.from_dir.clone() {
        return sync_from_dir(args, state_path, baseline, dir);
    }

    let drift_path = args
        .from
        .clone()
        .unwrap_or_else(|| default_drift_path(&state_path));
    tracing::info!(?state_path, ?drift_path, "sync");
    ensure_distinct(&state_path, &drift_path, "--from")?;

    let current = StateEngine::load(&drift_path)?;

    println!("TAPROOT SYNC");
    println!("─────────────────────────────────────────");
    println!(
        "baseline:   sha256:{} ({})",
        baseline.hash,
        display_state_path(&state_path)
    );
    println!(
        "drift:      sha256:{} ({})",
        current.hash,
        display_state_path(&drift_path)
    );
    println!();

    let diffs = crate::diff::diff_states(&baseline.state, &current.state, false);
    if diffs.is_empty() {
        println!("no drift — states are identical");
        if !args.keep && std::fs::remove_file(&drift_path).is_ok() {
            println!("removed:    {}", display_state_path(&drift_path));
        }
        return Ok(());
    }

    println!(
        "drift ({} field{}):",
        diffs.len(),
        if diffs.len() == 1 { "" } else { "s" }
    );
    for d in &diffs {
        let marker = match d.kind {
            crate::diff::DiffKind::Added => "+",
            crate::diff::DiffKind::Removed => "-",
            crate::diff::DiffKind::Changed => "~",
        };
        println!("  {marker} {}", d.path);
        if let Some(e) = &d.expected {
            println!("      expected: {e}");
        }
        if let Some(a) = &d.actual {
            println!("      actual:   {a}");
        }
        println!("      severity: {:?}", d.severity);
    }
    println!();

    if args.dry_run {
        println!("dry-run — nothing adopted.");
        println!("[re-run without --dry-run to sign and adopt]");
        return Ok(());
    }

    // Env-var drift is the intended writable surface — adopting it is the
    // point of sync. Refuse identity drift unless --force: that usually
    // means the drift file belongs to a different repo or baseline.
    // Keyed on path, not severity: base.branch/base.commit are only Warning
    // under non-strict diffing but still identity.
    let identity_drift: Vec<&str> = diffs
        .iter()
        .filter(|d| {
            d.path.starts_with("base.")
                || d.path.starts_with("runtimes.")
                || d.path.starts_with("containers.")
                || d.path == "version"
                || d.path == "notes"
        })
        .map(|d| d.path.as_str())
        .collect();
    if !identity_drift.is_empty() && !args.force {
        eprintln!(
            "✗ drift touches non-env fields ({}) — refusing to adopt without --force",
            identity_drift.join(", ")
        );
        return Err(TaprootError::Drift {
            breaking: identity_drift.len(),
            warning: diffs.len() - identity_drift.len(),
        });
    }

    let signed_new = sign_for_adoption(current.state.clone(), args.no_sign)?;
    StateEngine::save(&state_path, &signed_new)?;
    if !args.keep {
        std::fs::remove_file(&drift_path)?;
    }
    println!("adopted:    sha256:{}", signed_new.hash);
    if signed_new.signature.is_none() {
        print_unsigned_warning();
    }
    println!("path:       {}", display_state_path(&state_path));
    println!();
    print_status_line(true);
    Ok(())
}

/// Materialize-tree sync: read the edited `env` out of a `--no-fuse` tree,
/// capture it as drift, then hand off to the normal `--from` flow so review,
/// `--force` gating, signing, and adoption stay in one place.
fn sync_from_dir(
    mut args: SyncArgs,
    state_path: PathBuf,
    baseline: crate::state::SignedState,
    dir: PathBuf,
) -> Result<(), TaprootError> {
    let env_path = dir.join("env");
    if !env_path.exists() {
        eprintln!(
            "✗ no env file at {} — is this a materialized tree? (run `taproot mount --no-fuse` first)",
            env_path.display()
        );
        return Err(TaprootError::Mount(format!(
            "no env file in {}",
            dir.display()
        )));
    }

    let Some(drift) = crate::mount::capture_drift_from_dir(&dir, &baseline)? else {
        println!("TAPROOT SYNC");
        println!("─────────────────────────────────────────");
        println!("tree:       {}", display_state_path(&dir));
        println!();
        println!("no drift — env matches the signed baseline");
        return Ok(());
    };

    let drift_path = default_drift_path(&state_path);
    StateEngine::save(&drift_path, &drift)?;
    println!(
        "captured drift from {} → {}",
        dir.display(),
        drift_path.display()
    );

    args.from = Some(drift_path);
    handle_sync(SyncArgs {
        state_path: Some(state_path),
        from_dir: None,
        ..args
    })
}

pub fn handle_keys(args: KeysArgs) -> Result<(), TaprootError> {
    match args.command {
        KeysCommands::Generate(a) => handle_keys_generate(a),
        KeysCommands::List(a) => handle_keys_list(a),
        KeysCommands::Show(a) => handle_keys_show(a),
        KeysCommands::Rotate(a) => handle_keys_rotate(a),
    }
}

pub fn handle_keys_generate(args: KeysGenerateArgs) -> Result<(), TaprootError> {
    let keys_path = resolve_keys_path(args.keys);
    let ks = crate::keys::KeyStore::init(&keys_path)?;
    let kp = ks.generate(args.id)?;
    println!("TAPROOT KEYS GENERATE");
    println!("─────────────────────────────────────────");
    println!("id:         {}", kp.id);
    println!("pubkey:     {}", kp.public_key);
    println!("path:       {}/{}", display_state_path(&keys_path), kp.id);
    println!();
    println!("✓ generated — store private key securely, pubkey is shareable");
    Ok(())
}

pub fn handle_keys_list(args: KeysListArgs) -> Result<(), TaprootError> {
    let keys_path = resolve_keys_path(args.keys);
    let ks = crate::keys::KeyStore::init(&keys_path)?;
    let list = ks.list()?;
    println!("TAPROOT KEYS LIST");
    println!("─────────────────────────────────────────");
    println!("keys:       {}", display_state_path(&keys_path));
    if list.is_empty() {
        println!("(no keys — run `taproot keys generate`)");
    } else {
        for k in &list {
            let active = if k.active { "active" } else { "inactive" };
            println!(
                "  {}  {}  {active}  {}",
                k.id,
                &k.public_key[..16],
                k.created_at
            );
        }
    }
    Ok(())
}

pub fn handle_keys_show(args: KeysShowArgs) -> Result<(), TaprootError> {
    let keys_path = resolve_keys_path(args.keys);
    let ks = crate::keys::KeyStore::init(&keys_path)?;
    let kp = ks.get(&args.id)?;
    println!("{}", serde_json::to_string_pretty(&kp).unwrap());
    Ok(())
}

pub fn handle_keys_rotate(args: KeysRotateArgs) -> Result<(), TaprootError> {
    let keys_path = resolve_keys_path(args.keys);
    let ks = crate::keys::KeyStore::init(&keys_path)?;
    let kp = ks.rotate(args.deactivate_old)?;
    println!("✓ rotated — new key {}", kp.id);
    println!("  pubkey: {}", kp.public_key);
    if args.deactivate_old {
        println!("  old keys deactivated");
    }
    Ok(())
}

pub fn handle_fabric(args: FabricArgs) -> Result<(), TaprootError> {
    match args.command {
        FabricCommands::Audit(a) => handle_fabric_audit(a),
        FabricCommands::PolicyGet(a) => handle_fabric_policy_get(a),
        FabricCommands::PolicySet(a) => handle_fabric_policy_set(a),
        FabricCommands::TokenAdd(a) => handle_fabric_token_add(a),
        FabricCommands::TokenList(a) => handle_fabric_token_list(a),
    }
}

pub fn handle_fabric_audit(args: FabricAuditArgs) -> Result<(), TaprootError> {
    let fabric_path = resolve_fabric_path(args.fabric);
    let registry_path = resolve_registry_path(args.registry);
    let fabric = crate::fabric::Fabric::init(&fabric_path, &registry_path)?;
    let entries = fabric.audit_log(args.repo.as_deref())?;
    println!("TAPROOT FABRIC AUDIT");
    println!("─────────────────────────────────────────");
    if entries.is_empty() {
        println!("(no audit entries)");
    } else {
        for e in &entries {
            println!(
                "{}  {}  {}/{}  {}  signed={}",
                e.ts,
                e.action,
                e.repo,
                e.branch,
                &e.hash[..12],
                e.signed
            );
        }
        println!();
        println!("{} entries", entries.len());
    }
    Ok(())
}

pub fn handle_fabric_policy_get(args: FabricPolicyGetArgs) -> Result<(), TaprootError> {
    let fabric_path = resolve_fabric_path(args.fabric);
    let registry_path = resolve_registry_path(args.registry);
    let fabric = crate::fabric::Fabric::init(&fabric_path, &registry_path)?;
    let p = fabric.get_policy(&args.repo)?;
    println!("{}", serde_json::to_string_pretty(&p).unwrap());
    Ok(())
}

pub fn handle_fabric_policy_set(args: FabricPolicySetArgs) -> Result<(), TaprootError> {
    let fabric_path = resolve_fabric_path(args.fabric);
    let registry_path = resolve_registry_path(args.registry);
    let fabric = crate::fabric::Fabric::init(&fabric_path, &registry_path)?;
    let mut p = fabric.get_policy(&args.repo)?;
    p.repo = args.repo.clone();
    if let Some(v) = args.require_signed {
        p.require_signed = v;
    }
    if let Some(v) = args.require_check_strict {
        p.require_check_strict = v;
    }
    if !args.allow_branch.is_empty() {
        p.allowed_branches = args.allow_branch.clone();
    }
    if !args.block_env.is_empty() {
        p.blocked_env_keys = args.block_env.clone();
    }
    fabric.set_policy(&p)?;
    println!("✓ policy updated for {}", p.repo);
    println!("{}", serde_json::to_string_pretty(&p).unwrap());
    Ok(())
}

pub fn handle_fabric_token_add(args: FabricTokenAddArgs) -> Result<(), TaprootError> {
    let fabric_path = resolve_fabric_path(args.fabric);
    let registry_path = resolve_registry_path(args.registry);
    let fabric = crate::fabric::Fabric::init(&fabric_path, &registry_path)?;
    fabric.add_token(&args.token, &args.actor)?;
    println!("✓ token added for {}", args.actor);
    Ok(())
}

pub fn handle_fabric_token_list(args: FabricTokenListArgs) -> Result<(), TaprootError> {
    let fabric_path = resolve_fabric_path(args.fabric);
    let registry_path = resolve_registry_path(args.registry);
    let fabric = crate::fabric::Fabric::init(&fabric_path, &registry_path)?;
    let map = fabric.tokens()?;
    println!("TAPROOT TOKENS");
    println!("─────────────────────────────────────────");
    if map.is_empty() {
        println!("(no tokens — open registry)");
    } else {
        for (tok, actor) in &map {
            println!("  {actor:15}  {}...", &tok[..8.min(tok.len())]);
        }
    }
    Ok(())
}

pub fn handle_serve(args: ServeArgs) -> Result<(), TaprootError> {
    let registry_path = resolve_registry_path(args.registry);
    let fabric_path = resolve_fabric_path(args.fabric);
    println!("TAPROOT SERVE");
    println!("─────────────────────────────────────────");
    println!("registry:   {}", display_state_path(&registry_path));
    println!("fabric:     {}", display_state_path(&fabric_path));
    println!("addr:       {}", args.addr);
    println!();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| TaprootError::Io(std::io::Error::other(e.to_string())))?;
    rt.block_on(crate::server::serve(registry_path, fabric_path, args.addr))?;
    Ok(())
}

pub fn handle_remote(args: RemoteArgs) -> Result<(), TaprootError> {
    match args.command {
        RemoteCommands::Push(a) => handle_remote_push(a),
        RemoteCommands::Pull(a) => handle_remote_pull(a),
        RemoteCommands::Resolve(a) => handle_remote_resolve(a),
        RemoteCommands::Check(a) => handle_remote_check(a),
    }
}

fn with_auth(
    mut req: reqwest::blocking::RequestBuilder,
    token: Option<String>,
) -> reqwest::blocking::RequestBuilder {
    if let Some(tok) = token {
        req = req.header("Authorization", format!("Bearer {tok}"));
    }
    req
}

fn ensure_success(
    resp: reqwest::blocking::Response,
    ctx: &str,
) -> Result<reqwest::blocking::Response, TaprootError> {
    if !resp.status().is_success() {
        let txt = resp.text().unwrap_or_default();
        return Err(TaprootError::InvalidKey(format!("{ctx} failed: {txt}")));
    }
    Ok(resp)
}

pub fn handle_remote_push(args: RemotePushArgs) -> Result<(), TaprootError> {
    let state_path = resolve_state_path(args.state_path);
    let bytes = std::fs::read(&state_path)?;
    let signed: crate::state::SignedState = serde_json::from_slice(&bytes)?;
    crate::engine::StateEngine::verify(&signed)?;
    let url = format!("{}/v1/states", args.remote.trim_end_matches('/'));
    let client = reqwest::blocking::Client::new();
    let req = with_auth(client.post(&url).json(&signed), args.token);
    let resp = ensure_success(
        req.send()
            .map_err(|e| TaprootError::InvalidKey(e.to_string()))?,
        "remote push",
    )?;
    let v: serde_json::Value = resp
        .json()
        .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
    println!("✓ remote push ok — {}", v);
    Ok(())
}

pub fn handle_remote_pull(args: RemotePullArgs) -> Result<(), TaprootError> {
    let url = format!(
        "{}/v1/states/{}",
        args.remote.trim_end_matches('/'),
        args.hash
    );
    let client = reqwest::blocking::Client::new();
    let req = with_auth(client.get(&url), args.token);
    let resp = ensure_success(
        req.send()
            .map_err(|e| TaprootError::InvalidKey(e.to_string()))?,
        "remote pull",
    )?;
    let signed: crate::state::SignedState = resp
        .json()
        .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
    crate::engine::StateEngine::verify(&signed)?;
    if let Some(out) = args.out {
        crate::engine::StateEngine::save(&out, &signed)?;
        println!(
            "✓ remote pull {} -> {}",
            signed.hash,
            display_state_path(&out)
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&signed).unwrap());
    }
    Ok(())
}

pub fn handle_remote_resolve(args: RemoteResolveArgs) -> Result<(), TaprootError> {
    let repo = crate::registry::sanitize(&args.repo);
    let branch = crate::registry::sanitize(&args.branch);
    let url = format!(
        "{}/v1/refs/{}/{}",
        args.remote.trim_end_matches('/'),
        repo,
        branch
    );
    let client = reqwest::blocking::Client::new();
    let req = with_auth(client.get(&url), args.token);
    let resp = ensure_success(
        req.send()
            .map_err(|e| TaprootError::InvalidKey(e.to_string()))?,
        "remote resolve",
    )?;
    let v: serde_json::Value = resp
        .json()
        .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
    println!("{}", v["hash"].as_str().unwrap_or(""));
    Ok(())
}

pub fn handle_remote_check(args: RemoteCheckArgs) -> Result<(), TaprootError> {
    let url = format!("{}/v1/check", args.remote.trim_end_matches('/'));
    let client = reqwest::blocking::Client::new();
    let body = serde_json::json!({"baseline_hash": args.baseline_hash, "current_hash": args.current_hash, "strict": args.strict});
    let req = client.post(&url).json(&body);
    let resp = ensure_success(
        req.send()
            .map_err(|e| TaprootError::InvalidKey(e.to_string()))?,
        "remote check",
    )?;
    let v: serde_json::Value = resp
        .json()
        .map_err(|e| TaprootError::InvalidKey(e.to_string()))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        println!("drifted: {}  breaking: {}", v["drifted"], v["has_breaking"]);
        if let Some(diffs) = v["diffs"].as_array() {
            for d in diffs {
                println!("  {}: {}", d["path"], d["kind"]);
            }
        }
    }
    if v["has_breaking"].as_bool().unwrap_or(false) {
        return Err(TaprootError::Drift {
            breaking: 1,
            warning: 0,
        });
    }
    Ok(())
}

pub fn handle_check(args: CheckArgs) -> Result<(), TaprootError> {
    use crate::diff::{diff_states, has_breaking, CheckReport, EndpointInfo, Severity};

    let state_path = resolve_state_path(args.state_path);
    let baseline_path = args.baseline;

    tracing::info!(?state_path, ?baseline_path, "check");

    // A baseline that is the state under test compares a file to itself and
    // always reports no drift, which turns the gate into a no-op. Refuse it.
    ensure_distinct(&baseline_path, &state_path, "--baseline")?;

    // Load and verify both files — strict: unsigned is error
    let current = StateEngine::load(&state_path).map_err(|e| {
        eprintln!("✗ check failed — current state invalid: {e}");
        e
    })?;
    let baseline = StateEngine::load(&baseline_path).map_err(|e| {
        if !baseline_path.exists() {
            eprintln!(
                "hint: baseline not found at {}",
                display_state_path(&baseline_path)
            );
            return TaprootError::BaselineMissing(display_state_path(&baseline_path));
        }
        eprintln!("✗ check failed — baseline invalid: {e}");
        e
    })?;

    // Strict: unsigned states are not allowed (fail closed)
    // Both must be signed; otherwise treat as breaking drift
    let mut unsigned_warnings = Vec::new();
    if current.signature.is_none() {
        unsigned_warnings
            .push("current state is unsigned — not cryptographically signed".to_string());
    }
    if baseline.signature.is_none() {
        unsigned_warnings.push("baseline is unsigned — not cryptographically signed".to_string());
    }

    // Resolve strict: --no-strict overrides --strict
    let effective_strict = if args.no_strict { false } else { args.strict };
    let diffs = diff_states(&baseline.state, &current.state, effective_strict);

    // Count breaking vs warning
    let breaking = diffs
        .iter()
        .filter(|d| d.severity == Severity::Breaking)
        .count();
    let warnings = diffs
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();

    let has_unsigned_breaking = !unsigned_warnings.is_empty();
    let is_breaking_drift = breaking > 0 || has_unsigned_breaking;
    let is_any_drift = !diffs.is_empty() || has_unsigned_breaking;

    let report = CheckReport {
        version: "1.0".to_string(),
        baseline: EndpointInfo {
            path: baseline_path.display().to_string(),
            hash: baseline.hash.clone(),
            signed: baseline.signature.is_some(),
        },
        current: EndpointInfo {
            path: state_path.display().to_string(),
            hash: current.hash.clone(),
            signed: current.signature.is_some(),
        },
        drifted: is_any_drift,
        has_breaking: is_breaking_drift
            || (effective_strict && warnings > 0 && !args.allow_warnings),
        diffs: diffs.clone(),
        warnings: unsigned_warnings.clone(),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!("TAPROOT CHECK");
        println!("─────────────────────────────────────────");
        let b_short = if baseline.hash.len() >= 12 {
            &baseline.hash[..12]
        } else {
            &baseline.hash
        };
        let c_short = if current.hash.len() >= 12 {
            &current.hash[..12]
        } else {
            &current.hash
        };
        let b_sig = if baseline.signature.is_some() {
            "signed"
        } else {
            "unsigned"
        };
        let c_sig = if current.signature.is_some() {
            "signed"
        } else {
            "unsigned"
        };
        println!(
            "baseline:   {} ({b_sig} · sha256:{b_short})",
            display_state_path(&baseline_path)
        );
        println!(
            "current:    {} ({c_sig} · sha256:{c_short})",
            display_state_path(&state_path)
        );
        println!(
            "base:       {} {}@{} -> {} {}@{}",
            baseline.state.base.repo,
            baseline.state.base.branch,
            baseline.state.base.commit,
            current.state.base.repo,
            current.state.base.branch,
            current.state.base.commit,
        );
        println!();
        if diffs.is_empty() && unsigned_warnings.is_empty() {
            println!("drift:      none — no field drift");
            println!();
            println!("status:     ▶ INHERITED — no drift");
        } else {
            println!(
                "drift:      {breaking} breaking, {warnings} warning{}",
                if warnings == 1 { "" } else { "s" }
            );
            if !unsigned_warnings.is_empty() {
                for w in &unsigned_warnings {
                    println!("  ✗ {w} (breaking)");
                }
            }
            for d in &diffs {
                let icon = if d.severity == Severity::Breaking {
                    "✗"
                } else {
                    "⚠"
                };
                let sev = if d.severity == Severity::Breaking {
                    "breaking"
                } else {
                    "warning"
                };
                match d.kind {
                    crate::diff::DiffKind::Changed => {
                        let exp = d.expected.as_deref().unwrap_or("null");
                        let act = d.actual.as_deref().unwrap_or("null");
                        println!("  {icon} {}: {} -> {} (changed, {sev})", d.path, exp, act);
                    }
                    crate::diff::DiffKind::Added => {
                        let act = d.actual.as_deref().unwrap_or("");
                        println!("  {icon} {}: +{} (added, {sev})", d.path, act);
                    }
                    crate::diff::DiffKind::Removed => {
                        let exp = d.expected.as_deref().unwrap_or("");
                        println!("  {icon} {}: -{} (removed, {sev})", d.path, exp);
                    }
                }
            }
            println!();
            if is_breaking_drift || (effective_strict && warnings > 0) {
                println!(
                    "status:     ✗ DRIFTED — {} breaking",
                    breaking + unsigned_warnings.len()
                );
            } else {
                println!("status:     ⚠ DRIFTED — warnings only (pass with --allow-warnings or --no-strict)");
            }
        }
        println!();
    }

    // Strict exit logic: be as strict as possible
    // - unsigned => always fail (each unsigned counts as breaking)
    // - any breaking => fail
    // - warnings + strict => fail, warnings + allow_warnings => pass
    let unsigned_breaking = unsigned_warnings.len();
    if has_unsigned_breaking {
        return Err(TaprootError::Drift {
            breaking: breaking + unsigned_breaking,
            warning: warnings,
        });
    }
    if breaking > 0 {
        return Err(TaprootError::Drift {
            breaking,
            warning: warnings,
        });
    }
    if warnings > 0 && effective_strict && !args.allow_warnings {
        return Err(TaprootError::Drift {
            breaking,
            warning: warnings,
        });
    }
    debug_assert_eq!(has_breaking(&diffs), breaking > 0);

    Ok(())
}

// ---------------------------------------------------------------------------
// Registry handlers
// ---------------------------------------------------------------------------

pub fn handle_registry(args: RegistryArgs) -> Result<(), TaprootError> {
    match args.command {
        RegistryCommands::Push(a) => handle_registry_push(a),
        RegistryCommands::Pull(a) => handle_registry_pull(a),
        RegistryCommands::List(a) => handle_registry_list(a),
        RegistryCommands::Show(a) => handle_registry_show(a),
        RegistryCommands::Resolve(a) => handle_registry_resolve(a),
        RegistryCommands::Log(a) => handle_registry_log(a),
    }
}

pub fn handle_registry_push(args: RegistryPushArgs) -> Result<(), TaprootError> {
    let state_path = resolve_state_path(args.state_path);
    let registry_path = resolve_registry_path(args.registry);
    tracing::info!(?state_path, ?registry_path, "registry push");

    let signed = StateEngine::load(&state_path).map_err(|e| {
        eprintln!(
            "✗ registry push failed — state invalid at {}: {e}",
            display_state_path(&state_path)
        );
        e
    })?;

    // Policy check: if fabric policy exists and requires signed, reject unsigned locally too
    let fabric_path = resolve_fabric_path(None);
    if fabric_path.exists() {
        if let Ok(fabric) = crate::fabric::Fabric::init(&fabric_path, &registry_path) {
            let policy = fabric
                .get_policy(&signed.state.base.repo)
                .unwrap_or_default();
            if policy.require_signed && signed.signature.is_none() {
                eprintln!(
                    "✗ policy blocks unsigned push for repo {} (require_signed=true)",
                    signed.state.base.repo
                );
                return Err(TaprootError::InvalidKey(
                    "policy requires signed state".into(),
                ));
            }
        }
    }

    let registry = crate::registry::Registry::init(&registry_path)?;
    let hash = registry.push(&signed)?;

    // Audit local push as well (so local and remote are consistent)
    {
        let fabric_path = resolve_fabric_path(None);
        if let Ok(fabric) = crate::fabric::Fabric::init(&fabric_path, &registry_path) {
            let _ = fabric.audit(crate::fabric::AuditEntry {
                ts: chrono::Utc::now(),
                action: "push".into(),
                repo: signed.state.base.repo.clone(),
                branch: signed.state.base.branch.clone(),
                hash: hash.clone(),
                actor: "local".into(),
                signed: signed.signature.is_some(),
            });
        }
    }

    let short = if hash.len() >= 12 { &hash[..12] } else { &hash };
    let sig_label = if signed.signature.is_some() {
        "signed"
    } else {
        "unsigned"
    };
    println!("TAPROOT REGISTRY PUSH");
    println!("─────────────────────────────────────────");
    println!("repo:       {}", signed.state.base.repo);
    println!("branch:     {}", signed.state.base.branch);
    println!("hash:       {hash} (sha256:{short}, {sig_label})");
    println!("registry:   {}", display_state_path(&registry_path));
    println!(
        "object:     {}/objects/{hash}.json",
        display_state_path(&registry_path)
    );
    println!(
        "ref:        {}/refs/{}/{}",
        display_state_path(&registry_path),
        crate::registry::sanitize(&signed.state.base.repo),
        crate::registry::sanitize(&signed.state.base.branch)
    );
    println!();
    println!("✓ pushed — {sig_label} · sha256:{short}");
    Ok(())
}

pub fn handle_registry_pull(args: RegistryPullArgs) -> Result<(), TaprootError> {
    let registry_path = resolve_registry_path(args.registry);
    tracing::info!(hash=%args.hash, ?registry_path, "registry pull");

    let registry = crate::registry::Registry::init(&registry_path)?;
    let signed = registry.pull(&args.hash)?;

    if let Some(out) = args.out {
        StateEngine::save(&out, &signed)?;
        println!("✓ pulled {} -> {}", signed.hash, display_state_path(&out));
    } else {
        let short = if signed.hash.len() >= 12 {
            &signed.hash[..12]
        } else {
            &signed.hash
        };
        let sig_label = if signed.signature.is_some() {
            "signed"
        } else {
            "unsigned"
        };
        println!("TAPROOT REGISTRY PULL");
        println!("─────────────────────────────────────────");
        println!("hash:       {} ({sig_label} · sha256:{short})", signed.hash);
        println!("repo:       {}", signed.state.base.repo);
        println!(
            "base:       {}@{}",
            signed.state.base.branch, signed.state.base.commit
        );
        println!("registry:   {}", display_state_path(&registry_path));
        println!("runtimes:   {}", signed.state.runtimes.len());
        println!("containers: {}", signed.state.containers.len());
        println!("env-vars:   {}", signed.state.env_vars.len());
        if signed.signature.is_none() {
            print_unsigned_warning();
        }
        println!();
        // Also print state path hint
        println!(
            "[tip: taproot registry pull {} --out .taproot/state.json]",
            signed.hash
        );
    }
    Ok(())
}

pub fn handle_registry_list(args: RegistryListArgs) -> Result<(), TaprootError> {
    let registry_path = resolve_registry_path(args.registry);
    tracing::info!(repo=%args.repo, ?registry_path, "registry list");

    let registry = crate::registry::Registry::init(&registry_path)?;
    let entries = registry.list(&args.repo)?;

    println!("TAPROOT REGISTRY LIST");
    println!("─────────────────────────────────────────");
    println!("repo:       {}", args.repo);
    println!("registry:   {}", display_state_path(&registry_path));
    println!();
    if entries.is_empty() {
        println!("(no refs for repo {})", args.repo);
    } else {
        for (branch, hash) in &entries {
            let short = if hash.len() >= 12 { &hash[..12] } else { hash };
            println!("  {branch:20} {short}  {hash}");
        }
        println!();
        println!("{} branch(es)", entries.len());
    }
    Ok(())
}

pub fn handle_registry_show(args: RegistryShowArgs) -> Result<(), TaprootError> {
    // Alias for pull without writing — pretty-print SignedState JSON
    let registry_path = resolve_registry_path(args.registry);
    tracing::info!(hash=%args.hash, ?registry_path, "registry show");

    let registry = crate::registry::Registry::init(&registry_path)?;
    let signed = registry.pull(&args.hash)?;
    let json = serde_json::to_string_pretty(&signed).unwrap();
    println!("{json}");
    Ok(())
}

pub fn handle_registry_resolve(args: RegistryResolveArgs) -> Result<(), TaprootError> {
    let registry_path = resolve_registry_path(args.registry);
    tracing::info!(repo=%args.repo, branch=%args.branch, ?registry_path, "registry resolve");

    let registry = crate::registry::Registry::init(&registry_path)?;
    match registry.resolve_ref(&args.repo, &args.branch)? {
        Some(hash) => {
            println!("{hash}");
            Ok(())
        }
        None => {
            eprintln!(
                "ref not found: {}/{} in {}",
                args.repo,
                args.branch,
                display_state_path(&registry_path)
            );
            Err(TaprootError::RefNotFound {
                repo: args.repo,
                branch: args.branch,
            })
        }
    }
}

pub fn handle_registry_log(args: RegistryLogArgs) -> Result<(), TaprootError> {
    let registry_path = resolve_registry_path(args.registry);
    tracing::info!(repo=%args.repo, branch=%args.branch, ?registry_path, "registry log");

    let registry = crate::registry::Registry::init(&registry_path)?;
    let entries = registry.log(&args.repo, &args.branch)?;

    println!("TAPROOT REGISTRY LOG");
    println!("─────────────────────────────────────────");
    println!("repo:       {}", args.repo);
    println!("branch:     {}", args.branch);
    println!("registry:   {}", display_state_path(&registry_path));
    println!();
    if entries.is_empty() {
        println!("(no entries for {}/{})", args.repo, args.branch);
    } else {
        for (i, signed) in entries.iter().enumerate() {
            let short = if signed.hash.len() >= 12 {
                &signed.hash[..12]
            } else {
                &signed.hash
            };
            let sig_label = if signed.signature.is_some() {
                "signed"
            } else {
                "unsigned"
            };
            // First entry is the current ref head; the rest are ancestors.
            let marker = if i == 0 { "*" } else { " " };
            println!(
                "{marker} sha256:{short}  {}@{}  {sig_label}",
                signed.state.base.branch, signed.state.base.commit
            );
            if let Some(notes) = &signed.state.notes {
                println!("    notes: {notes}");
            }
        }
        println!();
        println!("{} entr(ies), newest first", entries.len());
    }
    Ok(())
}
