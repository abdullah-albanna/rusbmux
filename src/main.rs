use anyhow::Context;
use clap::Parser;
use rusbmux::{
    cli::{Cli, Commands},
    daemon::LISTENER_PATH,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if let Ok(val) = std::env::var("RUST_LOG") {
        val
    } else {
        match cli.verbose {
            0 => "error",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }
        .to_string()
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

    match cli.command {
        None => {
            rusbmux::daemon::run(cli.socket)
                .await
                .context("daemon failed")?;
        }
        Some(Commands::AddDevice(args)) => {
            if let Err(err) =
                rusbmux::cli::run_add_device(args, cli.socket.unwrap_or(LISTENER_PATH.to_string()))
                    .await
            {
                tracing::error!(%err, "AddDevice failed");
                std::process::exit(1);
            }

            println!("Device Added");
        }
    }

    Ok(())
}
