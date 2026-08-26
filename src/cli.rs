use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use crate::engine::StateEngine;
use crate::error::TaprootError;
use crate::state::TaprootState;

const DEFAULT_STATE_PATH: &str = ".taproot/state.json";

fn default_state_path() -> PathBuf {
    PathBuf::from(DEFAULT_STATE_PATH)
}

fn resolve_state_path(input: Option<PathBuf>) -> PathBuf {
    input.unwrap_or_else(default_state_path)
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
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Repository name (e.g. myapp)
    #[arg(long)]
    pub repo: String,

    /// Branch name (e.g. main)
    #[arg(long)]
    pub branch: String,

    /// Commit hash (e.g. 9f3a2c1)
    #[arg(long)]
    pub commit: String,

    /// Path to state file (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Skip signing (store hash only, no ed25519 signature)
    #[arg(long = "no-sign", default_value_t = false)]
    pub no_sign: bool,
}

#[derive(Debug, Args)]
pub struct MountArgs {
    /// Path to mount (materialisation target)
    pub path: PathBuf,

    /// Path to state file (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,

    /// Disable FUSE mount — just print header and exit (useful in CI without FUSE)
    #[arg(long = "no-fuse", default_value_t = false)]
    pub no_fuse: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Path to state file (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Path to state file (default: .taproot/state.json)
    #[arg(long, value_name = "PATH")]
    pub state_path: Option<PathBuf>,
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
    if value.contains('/') || value.contains('\0') {
        return Err(TaprootError::InvalidKey(format!(
            "{field} must not contain '/' or null byte"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
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
        let (priv_key, _pub_key) = StateEngine::generate_keypair();
        StateEngine::sign(&state, &priv_key)?
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
    println!("path:       {}", state_path.display());
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
                state_path.display()
            );
            if !Path::new(&state_path).exists() {
                eprintln!("hint: run `taproot init --repo <repo> --branch <branch> --commit <commit>` first");
            }
            println!();
            print_status_line(false);
            println!();
            return Err(e);
        }
    };

    print_mount_header(&signed);
    println!();
    println!("mount:      {}", args.path.display());
    match std::fs::symlink_metadata(&args.path) {
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
    // Only print INHERITED after mount succeeds, otherwise misleading
    if args.no_fuse {
        print_status_line(true);
        println!();
    }

    if args.no_fuse {
        println!("(no-fuse — skipping FUSE mount)");
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
            print_status_line(false);
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
    println!("path:       {}", state_path.display());
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
                println!("  path: {}", state_path.display());
                print_unsigned_warning();
            } else {
                println!("✓ verified — sha256:{}", signed.hash);
                println!(
                    "  repo: {}  base: {}@{}",
                    signed.state.base.repo, signed.state.base.branch, signed.state.base.commit
                );
                println!("  path: {}", state_path.display());
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("✗ verification failed for {}: {e}", state_path.display());
            Err(e)
        }
    }
}
