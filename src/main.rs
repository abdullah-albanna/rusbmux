#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    rusbmux::daemon::run().await;
}
