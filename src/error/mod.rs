use thiserror::Error;

use crate::{handler::ResultCode, parser::device_mux::UsbDevicePacket, watcher::DeviceEvent};

#[derive(Debug, Error)]
pub enum RusbmuxError {
    #[cfg(feature = "nusb")]
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

    #[error("{0}")]
    Channel(#[from] ChannelError),

    #[error("Received an unexpected packet: {0}")]
    UnexpectedPacket(String),

    #[error("Invalid data: {0}")]
    InvalidData(&'static str),

    #[error("Value not found: {0:?}")]
    ValueNotFound(MissingFields),

    #[error("A device with the {0} id is not found")]
    DeviceNotFound(u64),

    #[error("The system probably doesn't support usb hotplug")]
    HotPlugNotSupported,

    #[error("Plist parse error: {0}")]
    Plist(#[from] plist::Error),

    #[error("Ran out of source port for connections")]
    RanOutofSourcePort,

    #[error("The device rejected the power assertion: {0}")]
    PowerAssertion(String),

    #[error("{0}")]
    Idevice(#[from] idevice::IdeviceError),

    #[cfg(feature = "rusb")]
    #[error("{0}")]
    RusbError(#[from] rusb::Error),
}

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("Couldn't receive on a channel, error: {0}")]
    UsbRecv(String),

    #[error("Couldn't send a usb packet on the channel")]
    UsbSend(Box<UsbDevicePacket>),

    #[error("Couldn't send a broadcast event on the channel")]
    BroadcastSend(DeviceEvent),
}

#[derive(Debug, Clone, Copy)]
pub enum MissingFields {
    PairRecordID,
    PairRecordData,
    DeviceID,
    PortNumber,
    SystemBUID,
}

impl MissingFields {
    pub fn result_code(&self) -> ResultCode {
        match self {
            Self::PairRecordID | Self::PairRecordData | Self::SystemBUID => {
                ResultCode::InvalidInput
            }
            Self::DeviceID => ResultCode::BadDeviceOrNoSuchFile,
            Self::PortNumber => ResultCode::BadCommand,
        }
    }
}

impl From<crossfire::SendError<UsbDevicePacket>> for RusbmuxError {
    fn from(e: crossfire::SendError<UsbDevicePacket>) -> Self {
        Self::Channel(ChannelError::UsbSend(Box::new(e.0)))
    }
}

impl From<crossfire::TrySendError<UsbDevicePacket>> for RusbmuxError {
    fn from(e: crossfire::TrySendError<UsbDevicePacket>) -> Self {
        Self::Channel(ChannelError::UsbSend(Box::new(e.into_inner())))
    }
}

impl From<tokio::sync::broadcast::error::SendError<DeviceEvent>> for RusbmuxError {
    fn from(e: tokio::sync::broadcast::error::SendError<DeviceEvent>) -> Self {
        Self::Channel(ChannelError::BroadcastSend(e.0))
    }
}

impl From<crossfire::RecvError> for RusbmuxError {
    fn from(e: crossfire::RecvError) -> Self {
        Self::Channel(ChannelError::UsbRecv(e.to_string()))
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
