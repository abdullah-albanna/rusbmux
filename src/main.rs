#[tokio::main]
async fn main() {
    rusbmux::daemon::run().await;
}
