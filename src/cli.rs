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
    // Allow '/' for org/repo and branch names like feat/foo, but block traversal and empty segments
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
