# Implementation Research: Taproot — FUSE Mount CLI + Signed State Fabric

## The Task
**What:** Taproot — *state inheritance fabric between VCS and CI*. `Environment-as-object: state you inherit, sign, and never reproduce.`
Wedge is a FUSE mount CLI that lazily materializes git repos as signed environment snapshots (`taproot mount ~/projects/myapp` → `python 3.11.4 pinned`, `node 20.5.0`, `postgres 15.3 container signed`, 12 env-vars, lazy 2.4 GB, block-on-drift with `[s]ync·[f]ork·[d]etach`).
**Stack:** Rust `2024 edition` / `1.90.0+`, `fuser 0.17.x–0.18.0` (pure-Rust, optional libfuse3), `tokio 1.48`, `clap 4.4`, `libfuse3/fuse3` kernel driver, OCI 1.1 referrers + Sigstore/Cosign bundles for signed registry. State serialization engine (Rust) already done per README; building wedge primitive next: FUSE mount CLI, GitHub Action + baseline check, signed state registry, managed fabric API.
**Constraints:** Linux-only at wedge, unprivileged mount on `ubuntu-latest` GitHub Actions runner, must not leave dangling mounts on crash, must survive 10k+ `getattr` storms on `ls`/`prompt`, must preserve kernel permission semantics, must sign with content-addressable proof (Sigstore), must stay MIT-auditable for mount/protocol/schema.

---

## 1. Common Gotchas

### FUSE / fuser Gotchas (most load-bearing)

- **UID/GID must be real, not 0.** Set `FileAttr.uid/gid = getuid()/getgid()` — returning 0 with `DefaultPermissions` denies everything for uid 1000. Per-request `req.uid()/gid()` is for audit, not auth (unless `AllowOther`). Source: `reposix 06-gotchas.md §6.1` + `fuser Config` docs.
- **`st_size` must be exact.** Kernel short-circuits `read` at 0. For lazy state where size unknown until fetch, fetch metadata eagerly in `lookup` or return conservative estimate — never 0 for non-empty. After `setattr(resize)`, reply with `TTL=0` or next `stat` is stale. Source: `06-gotchas.md §6.2`.
- **Writes are chunked (≤128 KiB default) + `release` is commit boundary.** Editor saving 300 KiB → 3×`write` → `flush` → `release`. Do NOT ack upstream registry/Git until `release`. Accumulate per-fh buffer + batched flush on `release`. Optionally raise via `KernelConfig::set_max_write` in `init`. Use `tokio::sync::Mutex` per-file, not outer `RwLock`. Source: `06-gotchas.md §6.3`.
- **`AutoUnmount` is mandatory.** Without it, panic = dangling mount where `ls` hangs; recovery is `fusermount3 -u`. Use `BackgroundSession` via `spawn_mount2`; dropping session = unmount. Don't `let _ = spawn_mount2` (drops instantly). CI must `trap "fusermount3 -u /tmp/mnt" EXIT`. Source: `06-gotchas.md §6.4`.
- **Kernel caching TTLs bite freshness.** `ReplyEntry`/`ReplyAttr` TTL=0 = correct but slow; MAX = stale. Default **1s**. For remote state changes, use short TTL or `notifier().inval_inode()` / `FUSE_NOTIFY_INVAL_ENTRY`; otherwise `.taproot/refresh` pseudo-file pattern. Source: `06-gotchas.md §6.5`.
- **`getattr` on inode 1 (root) is hot path.** Every `cd`, tab-complete, prompt does `stat(CWD)`. Root attr must be cached constant, no write-lock. Source: `06-gotchas.md §6.7`.
- **"Disappearing file" after `create`.** Cause: `lookup` and `create` report different inodes or `getattr` returns ENOENT. Order: allocate inode → insert into `inodes` map → insert into parent `children` → reply. Never reply before inserting. Source: `06-gotchas.md §6.8`.
- **Permission checks with `DefaultPermissions`.** Kernel checks `mode+uid+gid` before calling you. `0o644` owned by 1000 is unwritable by 1001 — you never see `write()`. Use `DefaultPermissions` so mode bits are real (maps to RBAC-to-POSIX). Without it you must implement `access()` yourself. Source: `06-gotchas.md §6.9`.
- **MountOption comma/backslash escaping is broken if naive.** `FSName("foo,rw")` becomes two options. `fuser` must escape `,`→`\,` and `\`→`\\` for `FSName`; `Subtype` cannot contain `, \ NUL` reliably with libfuse; `CUSTOM` cannot contain `, NUL`. `fuser` currently panics on NUL — should error not panic. Source: `cberner/fuser issue #424`.
- **Error codes must be negative.** Return `-ENOENT` (i32 negative). Returning positive with empty payload = kernel nukes mount `107 Transport endpoint not connected`. Source: `fuser-iouring blog 2026-04-11`.
- **ABI version negotiation can freeze terminal.** Responding INIT with 7.32 advertises `READDIRPLUS` (opcode 52). If you return `ENOSYS` for it, kernel loops forever. Fix: downgrade INIT to 7.1 until you implement READDIRPLUS. Source: `fuser-iouring blog`.
- **Dcache phantom after `mkdir`→`rmdir`→`ls`.** Even TTL=0 leaves ghost if `rmdir`+`ls` race. Fix: bump parent `mtime` + set `size = children.len()` as version integer on every mutation so dcache invalidates. Prefer `FUSE_NOTIFY_INVAL_ENTRY` when available. Source: `fuser-iouring blog`.

### Async Bridge Gotcha
- **fuser 0.17/0.18 `Filesystem` is sync `&self` + `Send+Sync+'static`.** Don't use experimental `AsyncFilesystem` (shape churns per release). Recommended bridge: own a `tokio::runtime::Runtime` inside FS struct + `rt.block_on(...)` or `oneshot` roundtrip inside each callback. Pin `fuser = "0.17"` with `default-features = false` for Linux without `-dev` needed. Source: `reposix fuse-rust-patterns index + 05-async-bridge` + `fuser CHANGELOG 0.17.0` + `docs.rs fuser 0.18`.

### Signed Registry Gotchas
- **Attestations >4 MiB break OCI manifest push.** Registry `SHOULD` enforce manifest limit; blob endpoints support chunked large payloads. Cosign/Sigstore bundle spec specifically stores bundle as blob + referrer manifest with `subject` pointing to image digest — don't embed large SBOM in manifest annotations. Keep annotations <40 KiB (100 descriptors × 4 MiB). Source: `sigstore/cosign issue #3577` + `BUNDLE_SPEC.md`.
- **Referrers API vs tag fallback race.** GHCR/ECR/ACR/Harbor/Docker Hub support OCI 1.1 referrers as of 2025, but old registries fallback to `sha256-…` referrers tag schema which is read-append-write — concurrent pushes can drop entries. Native Referrers API has no race. `go-containerregistry` auto-fallbacks; don't manually maintain both. Source: `oci-encoding-format docs` + `safeguard 2026 snapshot`.
- **`COSIGN_REPOSITORY` redirect splits signature location.** If set, signatures live in different repo than image — discovery must resolve digest first. Prefer same-repo storage via OCI 1.1 referrers (`artifactType: application/vnd.dev.sigstore.bundle.v0.3+json`) for Taproot's content-addressable proof. Source: `cosign SYSTEM_CONFIG registry_support`.

### Reproducibility / DX Gotchas
- **Docker reproducibility is a myth without pinning every transitive.** Even pinned `FROM debian:bookworm-20240513` + `apt-get update && upgrade` = non-deterministic (different bits, libc drift). Leaf-node pin doesn't pin transitive deps. Build same Dockerfile 2 weeks apart → different image. Source: `arxiv 2601.12811 §3.2.2`, `Stahnke TNStack 2026-02-07`, `charemma blog`.
- **Dev Containers solve drift but tax inner-loop.** Docker FS sharing on macOS/Windows kills large-repo `watch` performance (bind-mount vs volume matters), coupled to VS Code, image pinned but Dockerfile not. Source: `dev.to/libme 2026-08-13`.
- **Nix is bit-for-bit but language is alien.** `flake.lock` gives exact hash (`python 3.11.4` same bits on macOS/Linux), but Nix language + `langserver not found` onboarding + binary cache misses (LLVM rebuild) make it single-expert risk. Source: `libme`, `dozen-donuts`, `howardjohn lazy-dev-env 2024-10-22`.

---

## 2. Best Practices

### Rust / CLI
- **Edition 2024 + Rust 1.90 idioms, `clap` derive, `cargo-dist` style releases.** Pin `fuser = "=0.17.0"` or `"0.18"`; check `docs.rs/fuser` for `Filesystem` trait migration (now `&self`, typed newtypes/bitflags). Use `Config` structured API (not `Vec<MountOption>`) + `n_threads` for multiple event loops. Source: `fuser docs.rs 0.18`, `CHANGELOG 0.17.0`.
- **Structured `Config` + ACL handling.** Replace old `mount2(Vec<MountOption>)` with `fuser::Config { mount_point, n_threads, ... }`. Explicitly set `allow_root`/`allow_other` only if `auto_unmount` needed — changelog notes `allow_root|allow_other must be enabled when using auto_unmount`. Source: `CHANGELOG 0.17.0`.
- **Logging: `trace!` per-op, `info` default.** Per-callback `debug!` floods `grep -r` (10k lines). Default `RUST_LOG=taproot_fuse=info`. Source: `06-gotchas.md §6.6`.
- **Error handling: typed `Errno` replies, not panics.** 0.17 adds `typed error handling across request/reply APIs` — propagate `Errno::ENOENT` etc. Panic in callback = dangling mount. Source: `CHANGELOG 0.17.0`.

### FUSE Mount
- **Skeleton: pure-Rust without libfuse on Linux, `fuse3` runtime package only on CI.** `sudo apt-get install fuse3` (not `-dev`) on `ubuntu-latest` is sufficient if `default-features = false`. Require `pkg-config` at build. Source: `docs.rs fuser deps`.
- **Inode allocation: stable, never reuse quickly.** Use `u64` counter + `BTreeMap<Ino, Node>`; don't recycle inodes within TTL window or kernel dcache aliases stale entry. Source: `reposix 04-inode-allocation` (inferred from gotchas + fuser tests migration notes).
- **Mount inside GitHub Actions:** use `BackgroundSession` + `Config::n_threads=2` (one for event loop, one for async bridge), `fuse3` package preinstalled on `ubuntu-latest` runner, verify via `mount | grep fuse` and `trap`. Source: `reposix 02-github-actions-mount`.

### Signed State Registry
- **Use SOCI pattern: don't convert image, add index artifact.** SOCI Snapshotter proves lazy-load without build-time conversion — builds separate `SOCI index` next to OCI image, queried via OCI Reference Types / referrers API. Taproot should build `taproot-index` (env manifest) as sidecar referrer, not mutate git object. Preserves signatures. Source: `awslabs/soci-snapshotter README` (76% of startup is download, only 6.4% needed — Harter FAST'16).
- **Cosign + Sigstore bundle v0.3 over OCI 1.1 referrers.** Push env snapshot as `application/vnd.oci.image.manifest.v1+json` with `subject: {digest: sha256:<git-baseline>}` + `artifactType: application/vnd.dev.sigstore.bundle.v0.3+json`, `config: empty descriptor`, `layers: [bundle blob]`. Clients discover via `GET /v2/<repo>/referrers/<digest>` or fallback tag. Works on all major registries 2025+. Source: `BUNDLE_SPEC.md` + `safeguard 2026`.
- **Keyless via Fulcio/Rekor in CI, hardened builder attestation.** Target: every production snapshot has signed SLSA 3 provenance, verified at admission (Kyverno/Policy Controller pattern — translate to `taproot verify` pre-exec). Source: `safeguard 2026` + `sigstore scaffolding`.

### Environment Reproducibility
- **Don't replay Dockerfile — capture derivation hash.** Nix motto: `few KB of code that produces GB is reproducibility; GB of hashes lying around is not` (Croughan via `dozen-donuts`). For Taproot: store content-addressed `sha256:<state>` where inputs = `flake.lock`/`Cargo.lock`/`package-lock.json` + provider (python/node/postgres) versions + env-vars, built in clean-room sandbox (no net). Source: `Stahnke`, `dozen-donuts`.
- **On-demand fetch, not eager.** Howard John's lazy-dev-env: eager `5-10 GB` kills onboarding. Taproot's wedge already proposes `lazy 2.4 GB` — materialize via FUSE `read` on-demand + SOCI-style prefetch window (first-N blocks). Combine with `nix run` shim + `direnv PATH_add ./bin` for binaries under `bin/`. Source: `howardjohn 2024-10-22`.

---

## 3. Pitfalls & Language Quirks

- **Rust: `&self` not `&mut self` now (0.17 breaking).** Filesystem impl must be `Send+Sync+'static` with interior mutability (`parking_lot::RwLock<BTreeMap>`). Old code using `&mut self` + exclusive lock will not compile. Source: `CHANGELOG 0.17.0`.
- **Rust: `allow_root`/`allow_other` gating `auto_unmount`.** Changelog: "`allow_root` or `allow_other` must be enabled when using `auto_unmount`" — if you want unprivileged mount with auto-cleanup, you must set one. Otherwise `mount2` fails. Source: `CHANGELOG 0.17.0`.
- **Rust: feature flag `libfuse` removed from defaults (0.16).** Linking with libfuse is now opt-in `features = ["libfuse"]`. Building without it gives pure-Rust backend (Linux only, handles mount via `/dev/fuse` directly). Mixing `libfuse` + pure-Rust in same workspace = double-mount confusion. Source: `CHANGELOG 0.16.0`.
- **Rust: `FUSERMOUNT_PATH` env override.** If `fusermount3` not in `PATH` (minimal CI image), set `FUSERMOUNT_PATH=/usr/bin/fusermount3`. Silent failure otherwise = `mount2` ioctl error. Source: `CHANGELOG 0.17.0`.
- **Quirk: kernel INIT max_write / max_pages negotiation.** 0.17 adds `max_pages`+`time_gran` in init; kernel may clamp `max_write` below your `KernelConfig`. Don't assume 128 KiB — check `init` reply's negotiated value and size your per-fh buffer accordingly. Source: `CHANGELOG 0.17.0`.
- **Quirk: `FUSE_DEV_IOC_CLONE` + passthrough fd (`BackingId`).** 0.17 adds passthrough descriptors (`ReplyCreate`/`ReplyOpen` with backing fd). Great for postgres `15.3 container, signed` path (pass through overlayfs fd) but leaks fd if you don't close on `release`. Source: `CHANGELOG 0.17.0`.
- **Quirk: `pkg-config` + `libfuse-dev` build coupling.** Even with `default-features=false`, build host still needs `pkg-config` crate's host `pkg-config` binary if any crate enables `libfuse`. CI must `apt-get install pkg-config` or build fails with opaque `failed to run pkg-config`. Source: `docs.rs fuser Linux deps`.
- **macOS: kext hell on Apple Silicon.** Requires `FUSE for macOS` + enable third-party kext (reboot, `csrutil`). Wedge should declare `linux-only` and provide `taproot check --dry-run` that validates without mounting on macOS. Don't promise macOS wedge v1. Source: `docs.rs fuser macOS`.
- **Silent failure: `TTL::MAX` on attrs hides drift.** If Taproot blocks execution on drifted state, stale `getattr` cache means process runs with old env-vars. Must `inval_inode` on `taproot sync` or use `TTL::ZERO` for env-file inodes. Source: `06-gotchas §6.5` + README "If the state has drifted from the signed baseline, Taproot blocks execution and offers a sync."
- **Nix interop quirk: `nix run` shebang + `direnv` exec overhead.** Howard's shim (`nix run` wrapping binary in `bin/`) adds ~70ms per exec cold (nix eval). For `python 3.11.4 (pinned)` hot path, cache resolved `nix store path` in `bin/python → exec /nix/store/…/bin/python` symlink after first fetch, not re-eval each call. Source: `howardjohn`.

---

## 4. Differentiation

**Industry standard — how environment reproducibility is *normally* done (2026):**

| Approach | Mechanism | Trust model | Reproducibility guarantee | Cost |
|---|---|---|---|---|
| Dockerfile + Registry | `FROM …; RUN apt-get` → `docker push` → digest pinned | Trust registry + image digest | **Not reproducible:** rebuild ≠ same bits (Stahnke: leaf pin ≠ transitive; timestamps not epoch) | Low learning, high drift |
| Dev Container / Codespaces | `devcontainer.json` + `Dockerfile` + Features | Same as Docker | Same as Docker + `updateContentCommand` drift | Medium, VS Code coupled, Mac FS tax |
| Nix Flake (`flake.lock`) | Pure function of inputs → `/nix/store/<hash>-pkg` | Hash of full build graph, sandbox no-net, epoch timestamps | **Bit-for-bit** (same inputs → same bits) | High Nix language tax, single-expert risk |
| `asdf`/`pyenv`/`nvm` + Makefile | Per-lang version files + `make setup` | Trust lockfiles | Documents intent, doesn't enforce (PATH drift) | Lowest, but drift not eliminated |
| SOCI / Stargz Snapshotter | Sidecar index, lazy fetch from OCI image | OCI digest + sidecar index | Image unchanged, fetch on demand (76% time saved) | Requires snapshotter plugin |

Source: `libme matrix`, `Stahnke`, `arxiv 2601.12811`, `SOCI README`, `howardjohn`.

**Taproot's wedge — what's different:**

- **Inheritance, not recipe:** `taproot mount` presents *already-built state* as a FUSE view (like an object you inherit), not a recipe you replay. State is **signed object** (`sha256:b2c1…`) with Sigstore bundle as OCI referrer (`subject: main@9f3a2c1`). Drift = execution blocked (README: `status: ▶ INHERITED — ready` vs blocked+sync). No other tool blocks on drift at exec-time; Docker/Nix trust you to be *in* the right env, Taproot enforces it.
- **Lazy materialization as first-class:** 2.4 GB lazy, not eager `nix develop` or `docker pull`. Uses SOCI-inspired index + FUSE on-demand `read`/`open` (not containerd snapshotter, so works without Docker daemon, without Kubernetes). Howard John notes *all* existing reproducible envs eagerly fetch GBs; Taproot's edge is **on-demand binary fetch** (`howardjohn: 5-10 GB but <10% used`).
- **Signed state registry as source of truth, not Git:** Git is snapshot *basis* (`base: main@9f3a2c1`); truth is signed state registry (OCI referrer store) that can be audited. Git forgot environment; Taproot remembers it as object you can `fork`/`detach`. Contrast: Nix's store is local + binary cache; OCI registry is shared, auditable, with Rekor transparency log.
- **UX as mount, not shell:** `taproot mount ~/projects/myapp` vs `nix develop` / `devcontainer open` / `docker run`. FUSE mount survives shell escape, works with any editor/tool without `direnv` hook, and `s/f/d` (sync/fork/detach) models inheritance semantics directly.

**Does the difference translate to usefulness? Honest check:**

- **Yes, if you nail these two:** (1) **Block-on-drift** is unique usefulness — no competitor enforces env at exec boundary (Docker checks image at *build*, Nix at *shell entry*, Taproot at *every syscall* via FUSE). That catches the 68% "works on my machine" and 52% CI env-failures from README cites at runtime, not post-mortem. (2) **Lazy + signed** removes the "download 5 GB to fix typo" tax that makes Nix/devcontainers abandoned (Howard's pain) while keeping audit trail (Sigstore) that Docker alone lacks.

- **No, if you just rebuild Docker/Nix with a mount:** If Taproot's `state serialization` is just `tar` of `docker export` or a thin wrapper over `nix store path` without SOCI-style index and without Sigstore provenance, it's vanity — `flox activate` already does `same hash, cross-platform (x86 Mac + Linux ARM)` with `calculated` env (Stahnke/Flox) and `devenv` already caches. Saying "we follow SOCI indexing + Sigstore bundle spec exactly" is a valid *not different* conclusion for registry layer — **use the standard**, don't invent a new attestation format. Differentiation is in **mount semantics + enforcement**, not in reinventing OCI/Cosign.

- **Risk if differentiation is fake:** "Environment-as-object" sounds novel but could devolve into `git clone + nix develop + fusermount` glue. Kill that darling: propose explicit kernel-enforced boundary — FUSE `open` handler checks `sha256` vs `baseline` and returns `EPERM` with sync hint, not just log warning. That's the firewall that makes inheritance real, not wiki 2.0.

**Version pin for this assessment:** Rust 1.90 (2024 edition), `fuser 0.17.0 (2026-02-14) → 0.18.0 (2026-07-22)`, `libfuse 3.10.3`, `tokio 1.48`, Sigstore Bundle `v0.3` + OCI Image Spec `1.1` Referrers API (2025 GA).

---

## Recommendation

**What to actually build (grounded in above):**

1. **Foundation: `taproot-mount` in Rust, `fuser 0.17` sync trait + tokio bridge, `Config` API, `AutoUnmount` + `DefaultPermissions` + 1s TTL + `BackgroundSession`.** Scaffold `src/fs.rs` implementing `Filesystem` with interior `RwLock<NodeTable>`; root attr constant; `lookup` eagerly fetches metadata from local state store (serialized snapshot) or registry index; `read` does lazy block fetch (SOCI-style 128 KiB chunks + prefetch). Add `taproot check` dry-run for non-Linux. Pin `fuser = "=0.17.0"` (or `0.18` if you absorb `&self` migration now). `pkg-config` in CI.

2. **State store: content-addressed, epoch-timestamped, sandbox-built.** Reuse existing Rust state serialization engine; add `state = sha256(filegraph + env-vars + provider versions)` with deterministic serialization (sort keys, epoch mtime). Store under `~/.taproot/store/<sha256>/`. This is the object you inherit — not Dockerfile replay. Verify with `sha256:b2c1…` on mount. Borrow Nix's sandbox+epoch principle without requiring Nix language.

3. **Signed registry: OCI 1.1 referrer + Sigstore bundle, not custom.** Push `taproot snapshot` as empty-config manifest with `subject: git baseline digest` + `artifactType: application/vnd.dev.sigstore.bundle.v0.3+json`. Use `cosign` lib (`sigstore-rs`) keyless via OIDC in GitHub Action. Don't store attestations in manifest — blob reference. Discovery via referrers API with fallback tag for old registries.

4. **GitHub Action + baseline check:** `taproot/action@v1` runs `taproot verify --baseline main@<sha>` before `cargo build`; fails CI with `drift detected: python expected 3.11.4 got 3.11.6 → taproot sync`. This closes the 52% CI env-failure loop.

5. **Arena-provable next:** Run arena on **mount semantics** (3 candidates: FUSE-only vs FUSE+overlayfs passthrough vs Git-filter fallback) — the foundation doc in `CD_res/implementation/taproot/arena-synthesis.md` captures graft. Don't write mount code until arena picks base — gate rule holds.

**Laziness Protocol:** smallest surface that proves inheritance — one `mount` that shows pinned `python/node/postgres` from signed snapshot and blocks drifted `env-var` exec. No `fork/detach` in wedge v1; they are graft candidates.

---

## Sources

- **Authoritative:** `docs.rs fuser 0.18.0` (cberner/fuser, 1273☆) — `Filesystem` trait, dependencies, platform deps, `mount2`/`Config` API. https://docs.rs/crate/fuser
- **Authoritative:** `fuser CHANGELOG 0.17.0 (2026-02-14)` + `0.16.0` — `&self` migration, `Config` replaces `Vec<MountOption>`, `allow_root|allow_other` gating `auto_unmount`, `libfuse` feature removal, `FUSERMOUNT_PATH`, `max_pages`/`FUSE_DEV_IOC_CLONE`. https://github.com/cberner/fuser/blob/master/CHANGELOG.md
- **Primary research:** `reposix 06-gotchas.md` (fuse-rust-patterns) — UID/GID, st_size, write buffering, AutoUnmount, kernel caching, getattr-hot, disappearing file, DefaultPermissions. High confidence on fuser 0.17. https://github.com/reubenjohn/reposix/blob/main/.planning/research/v0.1-fuse-era/fuse-rust-patterns/06-gotchas.md
- **Primary research:** `reposix index + 05-async-bridge + 02-github-actions-mount` — use `fuser 0.17.x default-features=false`, sync trait + tokio `rt.block_on`, `fuse3` on Actions runner. Same source tree.
- **Secondary:** `fuser-iouring blog 2026-04-11` — error code sign must be negative (107), ABI 7.32 READDIRPLUS loop, dcache phantom via `size=len`. https://blog.sdslabs.co/2026/04/fuser_iouring
- **Secondary:** `cberner/fuser issue #424 (2026-01-04)` — MountOption comma/backslash escaping, FSName vs Subtype vs CUSTOM handling. https://github.com/cberner/fuser/issues/424
- **Authoritative (OCI/Sigstore):** `sigstore/cosign BUNDLE_SPEC.md` — bundle as blob + manifest with `subject`, `artifactType v0.3`, empty config, referrers API. https://github.com/sigstore/cosign/blob/main/specs/BUNDLE_SPEC.md
- **Authoritative:** `cosign SYSTEM_CONFIG registry_support` + `SIGNATURE_SPEC` — OCI 1.1 referrers, `COSIGN_REPOSITORY` redirect, digest-tag `sha256-` index. https://docs.sigstore.dev/cosign/system_config/registry_support/
- **Authoritative:** `OCI Distribution Spec — Referrers Tag Schema + API` via `sigstore/cosign PR #2684` and `Tekton Chains oci-encoding-format` — `dsse` vs `sigstore-bundle`, fallback races, 4 MiB manifest limit, 40 KiB annotations. https://github.com/sigstore/cosign/pull/2684 + https://tekton.dev/docs/chains/oci-encoding-format/
- **Secondary (reproducibility):** `Malka et al. arXiv 2601.12811 §3.2` — Docker non-reproducibility causes, pinning tradeoffs. https://arxiv.org/html/2601.12811
- **Secondary:** `Stahnke / The New Stack 2026-02-07` — Docker reusable vs reproducible, Nix clean-room/epoch/no-net, Flox cross-platform `flox activate`. https://thenewstack.io/docker-versus-nix-the-quest-for-true-reproducibility/
- **Secondary:** `libme dev.to 2026-08-13` — Makefile vs devcontainers vs Nix matrix (setup, reproducibility, Mac FS tax). https://dev.to/libme/dev-environment-as-code-devcontainers-nix-or-just-a-good-makefile-3loi
- **Secondary:** `dozen-donuts.com 2024-05-30` + `charemma 2026-03-20` — `FROM debian:latest` drift, Croughan "KB code not GB hashes", pin leaf ≠ transitive. https://www.dozen-donuts.com/blog/02-nix-dev-env/
- **Secondary:** `SOCI Snapshotter (awslabs)` — 76% startup is download, 6.4% needed (Harter FAST'16), sidecar SOCI index without conversion, preserves signatures. https://github.com/awslabs/soci-snapshotter
- **Secondary:** `howardjohn 2024-10-22 lazy-dev-env` — eager 5-10 GB pain, Nix per-package container-like isolation, `nix run` + `bin/` shim + `direnv PATH_add`. https://blog.howardjohn.info/posts/lazy-dev-env/
- **Secondary:** `safeguard.sh 2026-02-03 OCI+CNCF 2026 snapshot` + `sigstore/cosign #3577` — OCI 1.1 GA 2025, bundle protobuf, `Fulcio/Rekor` prod, SLSA 3 provenance target. https://safeguard.sh/resources/blog/ocid-cncf-image-supply-chain-2026

---

## Adversarial Verification

- **Sources verified:** `docs.rs/fuser 0.18` + `CHANGELOG 0.17.0` exist (checked via webfetch; versions 0.18 2026-07-22, 0.17 2026-02-14). `reposix 06-gotchas.md` exists via websearch excerpt; `fuser issue #424` exists. `cosign BUNDLE_SPEC.md` + `registry_support` URLs resolve. `arxiv 2601.12811`, `thenewstack 2026-02-07`, `SOCI README`, `howardjohn` all returned highlights matching claims. One doc fetch 429 (exa MCP) ignored — claims cross-checked via other excerpts.
- **Numerical claims verified:** 76% download time / 6.4% needed — from Harter FAST'16 via SOCI README excerpt (exact). `max_write 128 KiB` default — from gotchas §6.3 excerpt. `fuser 1273 stars` — from docs.rs page header. 52%/68%/2.3 weeks figures — flagged as **README-cited** (DevOps Research 2026 etc.), not independently verified here; source is TAPROOT's research corpus `github.com/Epoch-AI-Lab/research`, not rechecked in this pass.
- **Logical coherence:** Confirmed — FUSE TTL tradeoff logic (short TTL vs inval) follows kernel dcache design; async bridge recommendation (sync trait + rt.block_on) follows 0.17 changelog "`Filesystem methods use &self, Send+Sync+'static` + experimental async unstable`. Differentiation "block-on-drift unique" follows from no other env tool hooking `open` to enforce `sha256` check — FUSE is only path that can intercept exec via filesystem, not just shell entry.
- **Omissions flagged:** Did not read `cdres/paper-hunting` corpus for environment drift papers beyond cited excerpts — manual verification should pull `DevOps Research 2026` + `CircleCI 2025` + `GitHub Octoverse 2026` sources before using stats externally. Did not test actual `fuser` mount on `ubuntu-latest` runner — recommend spike: `cargo new taproot-spike && cargo add fuser@0.17 && mount2` smoke test before committing to pure-Rust backend.
- **Status: GREEN** (after fixing: added `FUSERMOUNT_PATH` + `max_pages` negotiation + `BackingId` fd-leak pitfall from changelog re-read; added `artifactType` version `v0.3` correction from BUNDLE_SPEC).

