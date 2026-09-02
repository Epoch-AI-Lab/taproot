use taproot::cli::{
    handle_check, handle_init, handle_mount, handle_status, handle_sync, handle_verify, CheckArgs,
    InitArgs, MountArgs, SyncArgs,
};

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn init_allows_slash_in_repo_and_branch() {
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    let args = InitArgs {
        repo: "Epoch-AI-Lab/taproot".into(),
        branch: "feat/cli-v0.0.1-readonly-mount".into(),
        commit: "abc123".into(),
        state_path: Some(state_path.clone()),
        no_sign: true,
    };
    assert!(handle_init(args).is_ok());
    assert!(state_path.exists());
}

#[test]
fn init_rejects_dotdot() {
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    let args = InitArgs {
        repo: "myapp".into(),
        branch: "../etc".into(),
        commit: "abc".into(),
        state_path: Some(state_path),
        no_sign: true,
    };
    assert!(handle_init(args).is_err());
}

#[test]
fn init_rejects_empty_repo() {
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    let args = InitArgs {
        repo: "".into(),
        branch: "main".into(),
        commit: "abc".into(),
        state_path: Some(state_path),
        no_sign: true,
    };
    assert!(handle_init(args).is_err());
}

#[test]
fn mount_rejects_symlink_even_with_no_fuse() {
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    let init = InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(state_path.clone()),
        no_sign: true,
    };
    handle_init(init).unwrap();

    let real = dir.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let args = MountArgs {
        path: link,
        state_path: Some(state_path),
        no_fuse: true,
        drift_out: None,
    };
    assert!(handle_mount(args).is_err());
}

#[test]
fn mount_no_fuse_succeeds_on_valid_dir() {
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    let init = InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(state_path.clone()),
        no_sign: true,
    };
    handle_init(init).unwrap();

    let mnt = dir.path().join("mnt");
    std::fs::create_dir_all(&mnt).unwrap();

    let args = MountArgs {
        path: mnt,
        state_path: Some(state_path),
        no_fuse: true,
        drift_out: None,
    };
    assert!(handle_mount(args).is_ok());
}

#[test]
fn check_passes_on_identical_signed_states() {
    let dir = temp_dir();
    let baseline = dir.path().join("baseline.json");
    let head = dir.path().join("head.json");
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(baseline.clone()),
        no_sign: false,
    })
    .unwrap();
    std::fs::copy(&baseline, &head).unwrap();
    assert!(handle_check(CheckArgs {
        baseline: baseline.clone(),
        state_path: Some(head),
        json: false,
        strict: true,
        allow_warnings: false,
        no_strict: false,
    })
    .is_ok());
}

#[test]
fn check_fails_on_commit_drift_strict() {
    let dir = temp_dir();
    let baseline = dir.path().join("baseline.json");
    let head = dir.path().join("head.json");
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(baseline.clone()),
        no_sign: false,
    })
    .unwrap();
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "deadbeef".into(),
        state_path: Some(head.clone()),
        no_sign: false,
    })
    .unwrap();
    // strict=true => branch/commit drift is breaking
    assert!(handle_check(CheckArgs {
        baseline: baseline.clone(),
        state_path: Some(head.clone()),
        json: false,
        strict: true,
        allow_warnings: false,
        no_strict: false,
    })
    .is_err());
    // strict=false => warning only, should pass when allow_warnings false? actually warnings pass without strict
    // with allow_warnings=true and strict=true, warnings pass
    assert!(handle_check(CheckArgs {
        baseline,
        state_path: Some(head),
        json: false,
        strict: false,
        allow_warnings: false,
        no_strict: false,
    })
    .is_ok());
}

#[test]
fn check_fails_on_unsigned_strict() {
    let dir = temp_dir();
    let baseline = dir.path().join("baseline.json");
    let head = dir.path().join("head.json");
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(baseline.clone()),
        no_sign: false,
    })
    .unwrap();
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(head.clone()),
        no_sign: true,
    })
    .unwrap();
    assert!(handle_check(CheckArgs {
        baseline,
        state_path: Some(head),
        json: true,
        strict: true,
        allow_warnings: false,
        no_strict: false,
    })
    .is_err());
}

#[test]
fn check_fails_on_missing_baseline() {
    let dir = temp_dir();
    let head = dir.path().join("head.json");
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(head.clone()),
        no_sign: false,
    })
    .unwrap();
    assert!(handle_check(CheckArgs {
        baseline: dir.path().join("nope.json"),
        state_path: Some(head),
        json: false,
        strict: true,
        allow_warnings: false,
        no_strict: false,
    })
    .is_err());
}

#[test]
fn check_detects_env_drift() {
    use taproot::{StateEngine, TaprootState};
    let dir = temp_dir();
    let baseline = dir.path().join("baseline.json");
    let head = dir.path().join("head.json");
    // Create baseline with env FOO=bar
    let state = TaprootState::new("myapp", "main", "abc123").with_env("FOO", "bar");
    let (priv_key, _) = StateEngine::generate_keypair();
    let signed = StateEngine::sign(&state, &priv_key).unwrap();
    StateEngine::save(&baseline, &signed).unwrap();
    // Head adds env NEW=1
    let mut state2 = state.clone();
    state2.env_vars.insert("NEW".into(), "1".into());
    let signed2 = StateEngine::sign(&state2, &priv_key).unwrap();
    StateEngine::save(&head, &signed2).unwrap();
    assert!(handle_check(CheckArgs {
        baseline,
        state_path: Some(head),
        json: false,
        strict: true,
        allow_warnings: false,
        no_strict: false,
    })
    .is_err());
}

#[test]
fn status_and_verify_roundtrip() {
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    let init = InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(state_path.clone()),
        no_sign: false,
    };
    handle_init(init).unwrap();

    assert!(handle_status(taproot::cli::StatusArgs {
        state_path: Some(state_path.clone())
    })
    .is_ok());
    assert!(handle_verify(taproot::cli::VerifyArgs {
        state_path: Some(state_path)
    })
    .is_ok());
}

// ---------------------------------------------------------------------------
// sync — adopt drift, re-sign
// ---------------------------------------------------------------------------

fn sync_setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    let drift_path = dir.path().join("state.drift.json");
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(state_path.clone()),
        no_sign: false,
    })
    .unwrap();
    (dir, state_path, drift_path)
}

fn write_drift_from_env(state_path: &std::path::Path, drift_path: &std::path::Path, env: &str) {
    use taproot::mount::extract_env_drift;
    let baseline = taproot::StateEngine::load(state_path).unwrap();
    let drift = extract_env_drift(&baseline, env.as_bytes()).unwrap();
    taproot::StateEngine::save(drift_path, &drift).unwrap();
}

#[test]
fn sync_dry_run_reports_but_does_not_adopt() {
    let (_dir, state_path, drift_path) = sync_setup();
    write_drift_from_env(&state_path, &drift_path, "FOO=baz\nNEW=1\n");
    let before = taproot::StateEngine::load(&state_path).unwrap().hash;

    assert!(handle_sync(SyncArgs {
        state_path: Some(state_path.clone()),
        from: None,
        dry_run: true,
        no_sign: true,
        keep: false,
    })
    .is_ok());

    let after = taproot::StateEngine::load(&state_path).unwrap().hash;
    assert_eq!(before, after, "dry-run must not change state");
    assert!(drift_path.exists(), "dry-run must keep drift file");
}

#[test]
fn sync_adopts_drift_and_resigns() {
    let (_dir, state_path, drift_path) = sync_setup();
    let baseline_hash = taproot::StateEngine::load(&state_path).unwrap().hash;
    write_drift_from_env(&state_path, &drift_path, "FOO=baz\nNEW=1\n");

    assert!(handle_sync(SyncArgs {
        state_path: Some(state_path.clone()),
        from: None,
        dry_run: false,
        no_sign: true,
        keep: false,
    })
    .is_ok());

    let adopted = taproot::StateEngine::load(&state_path).unwrap();
    assert_eq!(adopted.state.env_vars.get("NEW").unwrap(), "1");
    assert_eq!(adopted.state.env_vars.get("FOO").unwrap(), "baz");
    assert_ne!(adopted.hash, baseline_hash);
    assert!(!drift_path.exists(), "drift file removed after sync");

    // check against the pre-sync baseline must now see no drift from adopted state
    // (baseline was replaced in place, so verify round-trips)
    assert!(handle_verify(taproot::cli::VerifyArgs {
        state_path: Some(state_path)
    })
    .is_ok());
}

#[test]
fn sync_errors_without_drift_file() {
    let (_dir, state_path, _drift_path) = sync_setup();
    assert!(handle_sync(SyncArgs {
        state_path: Some(state_path),
        from: None,
        dry_run: false,
        no_sign: true,
        keep: false,
    })
    .is_err());
}

#[test]
fn sync_identical_states_cleans_up_drift_file() {
    let (_dir, state_path, drift_path) = sync_setup();
    // drift with identical content — extract produces same env, but new
    // created_at; diff ignores created_at so this is "no drift"
    write_drift_from_env(&state_path, &drift_path, "FOO=bar\n");

    assert!(handle_sync(SyncArgs {
        state_path: Some(state_path),
        from: None,
        dry_run: false,
        no_sign: true,
        keep: false,
    })
    .is_ok());
    assert!(!drift_path.exists());
}
