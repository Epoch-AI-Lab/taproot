use taproot::cli::{
    handle_check, handle_init, handle_mount, handle_status, handle_verify, CheckArgs, InitArgs,
    MountArgs,
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
