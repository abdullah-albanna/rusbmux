#[tokio::main]
async fn main() {
    rusbmux::run_daemon().await;
}
