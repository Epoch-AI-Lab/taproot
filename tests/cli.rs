use std::path::PathBuf;
use taproot::cli::{handle_init, handle_mount, handle_status, handle_verify, InitArgs, MountArgs};

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
