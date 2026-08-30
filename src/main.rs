use clap::Parser;
use taproot::cli::{
    handle_check, handle_fabric, handle_init, handle_keys, handle_mount, handle_registry,
    handle_remote, handle_serve, handle_status, handle_verify, Cli, Commands,
};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init(args) => handle_init(args),
        Commands::Mount(args) => handle_mount(args),
        Commands::Status(args) => handle_status(args),
        Commands::Verify(args) => handle_verify(args),
        Commands::Check(args) => handle_check(args),
        Commands::Registry(args) => handle_registry(args),
        Commands::Keys(args) => handle_keys(args),
        Commands::Fabric(args) => handle_fabric(args),
        Commands::Serve(args) => handle_serve(args),
        Commands::Remote(args) => handle_remote(args),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
