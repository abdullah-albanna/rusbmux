use thiserror::Error;

#[derive(Debug, Error)]
pub enum RusbmuxError {
    #[error("USB error: {0}")]
    USB(#[from] nusb::Error),

    #[error("IO Error: {0}")]
    IO(#[from] std::io::Error),

    #[error("There is no usbmux interface found in the usb")]
    UsbmuxInterfaceNotFound,

    #[error("There is no bulk (in) endpoint found in the interface")]
    BulkInEndpointNotFound,

    #[error("There is no bulk (out) endpoint found in the interface")]
    BulkOutEndpointNotFound,

    #[error("{0}")]
    Parse(#[from] ParseError),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Received an unexpected packet: {0}")]
    UnexpectedPacket(String),

    #[error("Invalid data: {0}")]
    InvalidData(&'static str),

    #[error("value not found: {0}")]
    ValueNotFound(&'static str),

    #[error("A device with the {0} id is not found")]
    DeviceNotFound(u64),

    #[error("The system probably doesn't support usb hotplug")]
    HotPlug,

    #[error("Plist parse error: {0}")]
    Plist(#[from] plist::Error),

    #[error("Ran out of source port for connections")]
    RanOutofSourcePort,

    #[error("{0}")]
    Idevice(#[from] idevice::IdeviceError),
}

impl<T> From<crossfire::SendError<T>> for RusbmuxError {
    fn from(e: crossfire::SendError<T>) -> Self {
        Self::Channel(e.to_string())
    }
}

impl<T> From<crossfire::TrySendError<T>> for RusbmuxError {
    fn from(e: crossfire::TrySendError<T>) -> Self {
        Self::Channel(e.to_string())
    }
}

impl<T> From<tokio::sync::watch::error::SendError<T>> for RusbmuxError {
    fn from(e: tokio::sync::watch::error::SendError<T>) -> Self {
        Self::Channel(e.to_string())
    }
}

impl From<tokio::sync::watch::error::RecvError> for RusbmuxError {
    fn from(e: tokio::sync::watch::error::RecvError) -> Self {
        Self::Channel(e.to_string())
    }
}

impl<T> From<tokio::sync::broadcast::error::SendError<T>> for RusbmuxError {
    fn from(e: tokio::sync::broadcast::error::SendError<T>) -> Self {
        Self::Channel(e.to_string())
    }
}

impl From<crossfire::RecvError> for RusbmuxError {
    fn from(e: crossfire::RecvError) -> Self {
        Self::Channel(e.to_string())
    }
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("IO Error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Plist parse error: {0}")]
    Plist(#[from] plist::Error),

    #[error("Unable to parse tcp header: {0}")]
    TcpHeaderSlice(#[from] etherparse::err::tcp::HeaderSliceError),

    #[error("Invalid data: {0}")]
    InvalidData(String),
}
