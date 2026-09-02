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
        force: false,
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
        force: false,
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
        force: false,
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
        force: false,
        no_sign: true,
        keep: false,
    })
    .is_ok());
    assert!(!drift_path.exists());
}

#[test]
fn sync_refuses_from_pointing_at_state_file() {
    let (_dir, state_path, _drift_path) = sync_setup();
    let before = taproot::StateEngine::load(&state_path).unwrap().hash;
    assert!(handle_sync(SyncArgs {
        state_path: Some(state_path.clone()),
        from: Some(state_path.clone()),
        dry_run: false,
        force: false,
        no_sign: true,
        keep: false,
    })
    .is_err());
    // baseline must survive untouched
    let after = taproot::StateEngine::load(&state_path).unwrap().hash;
    assert_eq!(before, after);
}

#[test]
fn sync_refuses_identity_drift_without_force() {
    use taproot::{StateEngine, TaprootState};
    let (_dir, state_path, drift_path) = sync_setup();
    // drift for a DIFFERENT repo — self-consistent, but foreign
    let state = TaprootState::new("otherapp", "main", "abc123").with_env("FOO", "bar");
    let hash = StateEngine::hash(&state).unwrap();
    taproot::StateEngine::save(
        &drift_path,
        &taproot::SignedState {
            state,
            hash,
            signature: None,
            public_key: None,
        },
    )
    .unwrap();

    assert!(handle_sync(SyncArgs {
        state_path: Some(state_path.clone()),
        from: None,
        dry_run: false,
        force: false,
        no_sign: true,
        keep: false,
    })
    .is_err());
    // --force opts in
    assert!(handle_sync(SyncArgs {
        state_path: Some(state_path.clone()),
        from: None,
        dry_run: false,
        force: true,
        no_sign: true,
        keep: false,
    })
    .is_ok());
    let adopted = taproot::StateEngine::load(&state_path).unwrap();
    assert_eq!(adopted.state.base.repo, "otherapp");
}

#[test]
fn sign_state_with_keys_uses_stored_key() {
    use taproot::cli::sign_state_with_keys;
    use taproot::keys::KeyStore;
    let dir = temp_dir();
    let keys_root = dir.path().join("keys");
    KeyStore::init(&keys_root).unwrap();
    let kp = KeyStore::new(&keys_root).generate(None).unwrap();

    let state = taproot::TaprootState::new("myapp", "main", "abc123");
    let signed = sign_state_with_keys(state, false, &keys_root).unwrap();
    assert!(signed.signature.is_some());
    assert_eq!(signed.public_key.as_deref(), Some(kp.public_key.as_str()));
    taproot::StateEngine::verify(&signed).unwrap();
}

#[test]
fn sign_state_with_keys_ephemeral_and_no_sign() {
    use taproot::cli::sign_state_with_keys;
    let dir = temp_dir();
    let keys_root = dir.path().join("no-keys-here");

    let state = taproot::TaprootState::new("myapp", "main", "abc123");
    // no keystore on disk → ephemeral, still a valid signed state
    let signed = sign_state_with_keys(state.clone(), false, &keys_root).unwrap();
    assert!(signed.signature.is_some());
    assert!(signed.public_key.is_some());
    taproot::StateEngine::verify(&signed).unwrap();

    // --no-sign → hash only
    let unsigned = sign_state_with_keys(state, true, &keys_root).unwrap();
    assert!(unsigned.signature.is_none());
    taproot::StateEngine::verify(&unsigned).unwrap();
}

#[test]
fn extract_env_drift_rejects_non_utf8() {
    use taproot::mount::extract_env_drift;
    let dir = temp_dir();
    let state_path = dir.path().join("state.json");
    handle_init(InitArgs {
        repo: "myapp".into(),
        branch: "main".into(),
        commit: "abc123".into(),
        state_path: Some(state_path.clone()),
        no_sign: true,
    })
    .unwrap();
    let baseline = taproot::StateEngine::load(&state_path).unwrap();
    let raw = [0xFFu8, 0xFE, b'A', b'=', b'1'];
    assert!(extract_env_drift(&baseline, &raw).is_err());
}
