use tokio::sync::watch;

pub struct DeviceCore {
    pub id: u64,

    pub shutdown_tx: watch::Sender<()>,
    pub shutdown_rx: watch::Receiver<()>,
}

impl DeviceCore {
    #[must_use]
    pub fn new(id: u64) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(());
        Self {
            id,
            shutdown_tx,
            shutdown_rx,
        }
    }
}
