use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct DeviceCore {
    pub id: u64,

    pub canceler: CancellationToken,
}

impl DeviceCore {
    #[must_use]
    pub fn new(id: u64) -> Self {
        let canceler = CancellationToken::new();
        Self { id, canceler }
    }
}
