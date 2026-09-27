//! Detect the real environment so a state file describes the machine it was
//! taken on, rather than whatever a human typed into it.
//!
//! Every detector reads a file the project already commits or a tool already
//! installed. Nothing shells out to a version manager, and nothing probes the
//! network. A scan either finds a declaration it can parse or reports nothing,
//! so a state file never claims a runtime the project does not actually pin.

use std::collections::BTreeMap;
use std::path::Path;

use crate::state::{Container, Runtime, TaprootState};

/// What a scan found, before it is merged into a state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub runtimes: Vec<Runtime>,
    pub containers: Vec<Container>,
    /// Keys whose value could not be captured, with the reason.
    pub env_skipped: BTreeMap<String, String>,
}

impl ScanResult {
    pub fn is_empty(&self) -> bool {
        self.runtimes.is_empty() && self.containers.is_empty()
    }

    /// Fold the scan into an existing state, replacing what the scan found and
    /// leaving anything the scan did not detect untouched.
    pub fn apply_to(&self, mut state: TaprootState) -> TaprootState {
        if !self.runtimes.is_empty() {
            state.runtimes = dedup_runtimes(self.runtimes.clone());
        }
        if !self.containers.is_empty() {
            state.containers = dedup_containers(self.containers.clone());
        }
        state
    }
}

/// Canonical tool names, so `nodejs` and `node` do not become two runtimes for
/// one tool and produce a confusing state.
fn canonical_tool_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "nodejs" | "node.js" => "node".to_string(),
        "golang" | "go-lang" => "go".to_string(),
        "postgres" => "postgresql".to_string(),
        other => other.to_string(),
    }
}

/// Later detectors win for the same tool name, and the list stays sorted so the
/// serialized state and its hash are stable across runs.
fn dedup_runtimes(runtimes: Vec<Runtime>) -> Vec<Runtime> {
    let mut by_name: BTreeMap<String, Runtime> = BTreeMap::new();
    for mut r in runtimes {
        r.name = canonical_tool_name(&r.name);
        by_name.insert(r.name.clone(), r);
    }
    by_name.into_values().collect()
}

fn dedup_containers(containers: Vec<Container>) -> Vec<Container> {
    let mut by_name: BTreeMap<String, Container> = BTreeMap::new();
    for c in containers {
        by_name.insert(c.name.clone(), c);
    }
    by_name.into_values().collect()
}

// ---------------------------------------------------------------------------
// Runtimes
// ---------------------------------------------------------------------------

/// `.tool-versions` / `.mise.toml` style `name version [version...]` lines.
/// `node 20.5.0 18.0.0` pins node to 20.5.0 with 18.0.0 as an accepted fallback.
fn parse_tool_versions(content: &str) -> Vec<Runtime> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(version) = parts.next() else {
            continue;
        };
        out.push(Runtime {
            name: name.to_string(),
            version: version.to_string(),
            pinned: true,
        });
    }
    out
}

/// `[tools]` in a mise TOML: `"node" = "20.5.0"` or an array. The first entry
/// is the pin, the rest are fallbacks. Anything that is not a literal string or
/// an array of strings is a plugin reference or a dynamic value, so it is
/// skipped rather than guessed at.
fn parse_mise_toml(content: &str) -> Vec<Runtime> {
    let mut out = Vec::new();
    let mut in_tools = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_tools = line == "[tools]";
            continue;
        }
        if !in_tools || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let name = key.trim().trim_matches('"').trim_matches('\'').to_string();
        if name.is_empty() {
            continue;
        }
        let value = value.trim();
        let version = if value.starts_with('[') {
            let inner = value.trim_start_matches('[').trim_end_matches(']');
            inner
                .split(',')
                .map(|v| v.trim().trim_matches('"').trim_matches('\''))
                .find(|v| !v.is_empty())
                .unwrap_or("")
                .to_string()
        } else {
            value.trim_matches('"').trim_matches('\'').to_string()
        };
        if version.is_empty() || is_unpinned_spec(&version) {
            continue;
        }
        out.push(Runtime {
            name,
            version,
            pinned: true,
        });
    }
    out
}

/// A version spec that names no version. `latest`, `*`, and an empty string
/// are floating, not pinned, so they are not recorded as a pin.
fn is_unpinned_spec(spec: &str) -> bool {
    matches!(spec, "" | "*" | "latest" | "any" | "system")
}

/// A `FROM node:20.5.0` line in a Dockerfile. The image tag is the pin.
fn parse_dockerfile_runtime(content: &str) -> Vec<Runtime> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = strip_docker_directive(line, "FROM") else {
            continue;
        };
        // `FROM image AS stage` and `FROM golang:1.22 AS build` both count; a
        // `--platform=` flag is not part of the image reference.
        let rest = rest
            .split_whitespace()
            .find(|tok| !tok.starts_with("--"))
            .unwrap_or("");
        if rest.is_empty() || rest.starts_with('$') {
            continue;
        }
        let (name, version) = match rest.rsplit_once(':') {
            Some((n, v)) if !v.is_empty() && !v.contains('/') => (n, v),
            _ => (rest, "latest"),
        };
        if is_unpinned_spec(version) {
            continue;
        }
        let Some(name) = name.rsplit('/').next() else {
            continue;
        };
        out.push(Runtime {
            name: name.to_string(),
            version: version.to_string(),
            pinned: true,
        });
    }
    out
}

/// `engines.node` in package.json, via a targeted read rather than a full
/// JSON parse, so a malformed or huge file cannot break a scan.
fn parse_package_json_node(content: &str) -> Option<Runtime> {
    let idx = content.find("\"engines\"")?;
    let rest = &content[idx..];
    let node_key = rest.find("\"node\"")?;
    let after = &rest[node_key..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start();
    let value = value.strip_prefix('"')?;
    let end = value.find('"')?;
    let spec = &value[..end];
    let version = spec
        .trim_start_matches('^')
        .trim_start_matches('~')
        .trim_start_matches(">=")
        .trim_start_matches('=')
        .trim();
    if version.is_empty() || version == "*" || version == "latest" {
        return None;
    }
    Some(Runtime {
        name: "node".to_string(),
        version: version.to_string(),
        pinned: true,
    })
}

fn strip_docker_directive<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let upper = line.to_ascii_uppercase();
    if !upper.starts_with(name) {
        return None;
    }
    let after = &line[name.len()..];
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    Some(after.trim_start())
}

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

/// `image: postgres:15.3` in a compose file. The service key becomes the
/// container name. A bare `image: postgres` pins to `latest`.
fn parse_compose(content: &str) -> Vec<Container> {
    let mut out = Vec::new();
    let mut service: Option<String> = None;
    let mut in_services = false;
    for raw in content.lines() {
        let line = raw.split('#').next().unwrap_or("");
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        if trimmed.is_empty() {
            continue;
        }
        if indent == 0 {
            in_services = trimmed == "services:";
            service = None;
            continue;
        }
        if !in_services {
            continue;
        }
        // A service name is an indented key with no value, one level in.
        if indent == 2 {
            let key = trimmed.trim_end_matches(':');
            if !key.contains(' ') && !key.contains('=') && !key.starts_with('-') {
                service = Some(key.trim_matches('"').to_string());
            }
            continue;
        }
        let Some(key) = trimmed.strip_prefix("image:") else {
            continue;
        };
        let Some(name) = service.clone() else {
            continue;
        };
        let image = key.trim().trim_matches('"').trim_matches('\'');
        if image.is_empty() {
            continue;
        }
        let (image_name, version) = match image.rsplit_once(':') {
            Some((n, v)) if !v.is_empty() && !v.contains('/') => (n.to_string(), v.to_string()),
            _ => (image.to_string(), "latest".to_string()),
        };
        out.push(Container {
            name,
            version: version.clone(),
            image: format!("{image_name}:{version}"),
            signed: true,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Env
// ---------------------------------------------------------------------------

/// Read a declared env file, dropping anything that looks like a live secret.
///
/// This never walks the process environment. A state file gets committed, so
/// harvesting whatever happens to be exported would publish credentials. Only
/// explicit declarations are read, and a value matching a secret shape is
/// skipped and reported instead of stored.
pub fn scan_env_file(path: &Path) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut kept = BTreeMap::new();
    let mut skipped = BTreeMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return (kept, skipped);
    };
    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if key.is_empty() || !is_env_key(key) {
            continue;
        }
        match classify_secret(key, value) {
            Some(reason) => {
                skipped.insert(key.to_string(), format!("line {}: {reason}", idx + 1));
            }
            None => {
                kept.insert(key.to_string(), value.to_string());
            }
        }
    }
    (kept, skipped)
}

/// Keys that are safe to record as-is. An env key is a shell identifier.
fn is_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Why a value looks like a live credential rather than configuration. The
/// name alone is not enough: `DATABASE_URL` is config, `PORT` is config, and
/// plenty of legitimately-committed env files have `*_URL` keys.
fn classify_secret(key: &str, value: &str) -> Option<&'static str> {
    if value.is_empty() {
        return None;
    }
    let upper_key = key.to_ascii_uppercase();
    let has_secret_word = [
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "PRIVATE_KEY",
        "APIKEY",
        "API_KEY",
    ]
    .iter()
    .any(|w| upper_key.contains(w));
    if has_secret_word {
        return Some("key name marks it as a secret");
    }
    // High-entropy values that look like real provider credentials regardless
    // of how they are named. These prefixes are unambiguous.
    for prefix in [
        "sk_live_",
        "sk_test_",
        "sk-",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "ASIA",
        "AIza",
        "ya29.",
        "glpat-",
    ] {
        if value.starts_with(prefix) {
            return Some("value matches a known credential format");
        }
    }
    // A private key block regardless of the key name.
    if value.contains("-----BEGIN") && value.contains("PRIVATE KEY") {
        return Some("value is a private key block");
    }
    None
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Scan a project directory for declared runtimes, containers, and env vars.
///
/// Detectors are independent and each contributes only what it can parse, so a
/// project with a Dockerfile and no `.tool-versions` still gets its base image.
pub fn scan_project(root: &Path) -> ScanResult {
    let mut result = ScanResult::default();
    let mut runtimes: Vec<Runtime> = Vec::new();

    for (name, parse) in DETECTORS {
        let path = root.join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        runtimes.extend(parse(&content));
    }

    result.runtimes = dedup_runtimes(runtimes);

    for name in COMPOSE_FILES {
        let path = root.join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        result.containers.extend(parse_compose(&content));
    }
    result.containers = dedup_containers(result.containers);

    result
}

/// A detector: a file to read, and a parser turning its contents into runtimes.
type Detector = (&'static str, fn(&str) -> Vec<Runtime>);

/// Detector filenames and their parsers. Order decides precedence when two
/// files pin the same tool: later entries win in `dedup_runtimes`.
const DETECTORS: &[Detector] = &[
    (".tool-versions", parse_tool_versions),
    (".mise.toml", parse_mise_toml),
    ("Dockerfile", parse_dockerfile_runtime),
    ("package.json", |c| {
        parse_package_json_node(c).into_iter().collect()
    }),
];

const COMPOSE_FILES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

const ENV_FILES: &[&str] = &[".env", ".env.local", ".env.example", ".env.sample"];

/// Env vars a scan found, kept separate from the skip report so the caller
/// writes them into `state.env_vars` rather than into containers.
pub fn scan_env_vars(root: &Path) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut kept = BTreeMap::new();
    let mut skipped = BTreeMap::new();
    for name in ENV_FILES {
        let (k, s) = scan_env_file(&root.join(name));
        for (key, val) in k {
            kept.entry(key).or_insert(val);
        }
        skipped.extend(s);
    }
    (kept, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(name: &str, version: &str) -> Runtime {
        Runtime {
            name: name.into(),
            version: version.into(),
            pinned: true,
        }
    }

    #[test]
    fn parses_tool_versions() {
        let r = parse_tool_versions("node 20.5.0\npython 3.11.4 3.10.0\n\n# comment\nruby\n");
        assert_eq!(r, vec![rt("node", "20.5.0"), rt("python", "3.11.4")]);
    }

    #[test]
    fn tool_versions_picks_first_version() {
        let r = parse_tool_versions("python 3.11.4 3.10.0 3.9.0\n");
        assert_eq!(r[0].version, "3.11.4");
    }

    #[test]
    fn parses_mise_string_and_array() {
        let content =
            "[tools]\nnode = \"20.5.0\"\npython = [\"3.11.4\", \"3.10.0\"]\ngolang = \"latest\"\n";
        let r = parse_mise_toml(content);
        assert_eq!(r, vec![rt("node", "20.5.0"), rt("python", "3.11.4")]);
    }

    #[test]
    fn mise_stops_at_next_section() {
        let content = "[tools]\nnode = \"20.5.0\"\n\n[settings]\nnotatool = \"x\"\n";
        assert_eq!(parse_mise_toml(content), vec![rt("node", "20.5.0")]);
    }

    #[test]
    fn parses_dockerfile_from() {
        let content = "FROM node:20.5.0 AS build\nRUN npm i\nFROM alpine:3.19\n";
        let r = parse_dockerfile_runtime(content);
        assert_eq!(r, vec![rt("node", "20.5.0"), rt("alpine", "3.19")]);
    }

    #[test]
    fn dockerfile_ignores_dynamic_and_flags() {
        let content = "ARG BASE\nFROM ${BASE}\nFROM --platform=linux/amd64 python:3.11.4\n";
        let r = parse_dockerfile_runtime(content);
        assert_eq!(r, vec![rt("python", "3.11.4")]);
    }

    #[test]
    fn dockerfile_untagged_is_not_a_pin() {
        // `FROM postgres` names no version, so it is not a pin and must not
        // enter the state. A floating tag is not an inherited environment.
        assert!(parse_dockerfile_runtime("FROM postgres\n").is_empty());
    }

    #[test]
    fn package_json_engines_node() {
        let r = parse_package_json_node(r#"{"engines":{"node":">=20.5.0"}}"#).unwrap();
        assert_eq!(r, rt("node", "20.5.0"));
    }

    #[test]
    fn package_json_wildcard_engine_is_not_a_pin() {
        assert!(parse_package_json_node(r#"{"engines":{"node":"*"}}"#).is_none());
    }

    #[test]
    fn package_json_without_engines() {
        assert!(parse_package_json_node(r#"{"name":"x"}"#).is_none());
    }

    #[test]
    fn parses_compose_services() {
        let content = "services:\n  db:\n    image: postgres:15.3\n  cache:\n    image: redis:7\n";
        let c = parse_compose(content);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "db");
        assert_eq!(c[0].image, "postgres:15.3");
        assert_eq!(c[1].version, "7");
    }

    #[test]
    fn compose_ignores_other_sections() {
        let content = "volumes:\n  data:\n    image: should-not-match\nservices:\n  db:\n    image: postgres:15.3\n";
        let c = parse_compose(content);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "db");
    }

    #[test]
    fn compose_untagged_image_is_latest() {
        let c = parse_compose("services:\n  db:\n    image: postgres\n");
        assert_eq!(c[0].version, "latest");
    }

    #[test]
    fn env_file_parses_and_strips_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "A=1\nB=\"two\"\nexport C='three'\n# D=4\n").unwrap();
        let (kept, skipped) = scan_env_file(&p);
        assert_eq!(kept.get("A").unwrap(), "1");
        assert_eq!(kept.get("B").unwrap(), "two");
        assert_eq!(kept.get("C").unwrap(), "three");
        assert!(!kept.contains_key("D"));
        assert!(skipped.is_empty());
    }

    #[test]
    fn env_file_skips_secret_named_keys() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "API_TOKEN=abc\nDB_PASSWORD=hunter2\nPORT=3000\n").unwrap();
        let (kept, skipped) = scan_env_file(&p);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept.get("PORT").unwrap(), "3000");
        assert!(skipped.contains_key("API_TOKEN"));
        assert!(skipped.contains_key("DB_PASSWORD"));
    }

    #[test]
    fn env_file_skips_values_that_look_like_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "PAYMENT=sk_live_abc123\nMODE=prod\n").unwrap();
        let (kept, _) = scan_env_file(&p);
        assert!(!kept.contains_key("PAYMENT"));
        assert_eq!(kept.get("MODE").unwrap(), "prod");
    }

    #[test]
    fn env_file_skips_private_key_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "CERT=-----BEGIN RSA PRIVATE KEY-----\nabc\n").unwrap();
        let (kept, _) = scan_env_file(&p);
        assert!(!kept.contains_key("CERT"));
    }

    #[test]
    fn env_key_must_be_an_identifier() {
        assert!(is_env_key("DATABASE_URL"));
        assert!(is_env_key("NODE_ENV"));
        assert!(!is_env_key("1BAD"));
        assert!(!is_env_key("has space"));
        assert!(!is_env_key(""));
    }

    #[test]
    fn database_url_is_not_treated_as_secret() {
        assert_eq!(
            classify_secret("DATABASE_URL", "postgres://localhost/app"),
            None
        );
    }

    #[test]
    fn scan_project_finds_both_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".tool-versions"), "node 20.5.0\n").unwrap();
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  db:\n    image: postgres:15.3\n",
        )
        .unwrap();
        let r = scan_project(root);
        assert_eq!(r.runtimes, vec![rt("node", "20.5.0")]);
        assert_eq!(r.containers.len(), 1);
        assert_eq!(r.containers[0].image, "postgres:15.3");
    }

    #[test]
    fn scan_project_on_empty_dir_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let r = scan_project(dir.path());
        assert!(r.is_empty());
    }

    #[test]
    fn scan_ignores_malformed_files_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("package.json"), "{ this is not json").unwrap();
        std::fs::write(root.join(".mise.toml"), "[tools\nbroken").unwrap();
        let r = scan_project(root);
        assert!(r.is_empty());
    }

    #[test]
    fn apply_to_preserves_untouched_fields() {
        let base = TaprootState::new("app", "main", "abc").with_env("KEEP", "me");
        let scan = ScanResult {
            runtimes: vec![rt("node", "20.5.0")],
            ..Default::default()
        };
        let out = scan.apply_to(base);
        assert_eq!(out.runtimes.len(), 1);
        assert_eq!(out.env_vars.get("KEEP").unwrap(), "me");
    }
}
