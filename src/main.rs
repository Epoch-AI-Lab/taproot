use taproot::{StateEngine, TaprootState};

fn main() {
    // Minimal demo — will become `taproot mount` CLI in next step.
    let state = TaprootState::new("myapp", "main", "9f3a2c1")
        .with_runtime("python", "3.11.4")
        .with_runtime("node", "20.5.0")
        .with_container("postgres", "15.3", "postgres:15.3")
        .with_env("DATABASE_URL", "postgres://localhost/myapp");

    let hash = StateEngine::hash(&state).expect("hash");
    let (priv_key, pub_key) = StateEngine::generate_keypair();
    let signed = StateEngine::sign(&state, &priv_key).expect("sign");

    println!("TAPROOT STATE");
    println!("─────────────────────────────────────────");
    println!("repo:       {}", state.base.repo);
    println!("base:       {}@{}", state.base.branch, state.base.commit);
    println!("state:      signed · sha256:{}", &hash[..12]);
    println!("runtimes:   {}", state.runtimes.len());
    for r in &state.runtimes {
        println!("  - {}: {} (pinned={})", r.name, r.version, r.pinned);
    }
    println!("containers: {}", state.containers.len());
    println!("env-vars:   {}", state.env_vars.len());
    println!();
    println!("hash:       {}", signed.hash);
    println!("pubkey:     {}...", &pub_key[..16]);
    println!("verified:   {}", StateEngine::verify(&signed).is_ok());
    println!();
    println!("status:     ▶ INHERITED — ready to work");
    println!();
    println!("[next: cargo run -- mount ~/projects/myapp]");
}
