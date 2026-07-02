#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if let Err(e) = rusbmux::daemon::run().await {
        tracing::error!(err = ?e, "Daemon failed");
    }
}
