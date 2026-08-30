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
    /// Mount a taproot state (v0.0.1: read-only FUSE)
    Mount(MountArgs),
    /// Show current state status
    Status(StatusArgs),
    /// Verify state signature and hash
    Verify(VerifyArgs),
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
pub struct MountArgs {
    /// Path to mount (must be an existing empty directory)
    pub path: PathBuf,

    /// Path to state file (default: .taproot/state.json, relative to current directory)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Disable FUSE mount — just print header and exit (useful in CI without FUSE)
    #[arg(long = "no-fuse", default_value_t = false)]
    pub no_fuse: bool,
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
// Handlers
// ---------------------------------------------------------------------------

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
                Err(_) => (StateEngine::generate_keypair().0, None),
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
    println!("mount:      {}", args.path.display());
    let target_meta = std::fs::symlink_metadata(&args.path);
    match &target_meta {
        Ok(m) if m.is_dir() => println!("target:     exists (directory)"),
        Ok(m) if m.file_type().is_symlink() => {
            println!("target:     exists (symlink — will be rejected)")
        }
        Ok(_) => println!("target:     exists (not a directory — will be rejected)"),
        Err(_) => println!("target:     not found"),
    }
    println!("hash:       {}", signed.hash);
    if signed.signature.is_none() {
        print_unsigned_warning();
    }
    println!();

    // Validate mountpoint before honoring --no-fuse — CI must not hide symlink/file attacks
    if let Ok(m) = &target_meta {
        if m.file_type().is_symlink() {
            let e = TaprootError::Mount(format!(
                "mountpoint is a symlink (refusing): {}",
                args.path.display()
            ));
            eprintln!("✗ mount failed: {e}");
            println!("status:     ✗ MOUNT FAILED — symlink rejected");
            println!();
            return Err(e);
        }
        if !m.is_dir() {
            let e = TaprootError::Mount(format!(
                "mountpoint is not a directory: {}",
                args.path.display()
            ));
            eprintln!("✗ mount failed: {e}");
            println!("status:     ✗ MOUNT FAILED — not a directory");
            println!();
            return Err(e);
        }
    } else if !args.no_fuse {
        // real mount requires existing dir
        let e = TaprootError::Mount(format!(
            "mountpoint does not exist: {}",
            args.path.display()
        ));
        eprintln!("✗ mount failed: {e}");
        println!("status:     ✗ MOUNT FAILED — mountpoint missing");
        println!();
        return Err(e);
    }

    if args.no_fuse {
        println!("(no-fuse — skipping FUSE mount, mountpoint validated)");
        print_status_line(true);
        println!();
        return Ok(());
    }

    println!(
        "attempting FUSE mount at {} (read-only, Ctrl-C to unmount)...",
        args.path.display()
    );
    match crate::mount::mount_readonly(&args.path, &signed) {
        Ok(()) => {
            print_status_line(true);
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
    // Validate has_breaking helper stays consistent
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
        for signed in &entries {
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
            println!(
                "* {}  {}@{}  {sig_label} · sha256:{short}",
                signed.hash, signed.state.base.branch, signed.state.base.commit
            );
            if let Some(notes) = &signed.state.notes {
                println!("  notes: {notes}");
            }
        }
        println!();
        println!("{} entr(ies)", entries.len());
    }
    Ok(())
}
