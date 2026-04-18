use std::sync::Arc;

use bytes::Bytes;
pub mod network;
pub mod usb;

use crate::{
    device::ConectionType, error::RusbmuxError, parser::device_mux::UsbDevicePacket,
    usb::MAX_PACKET_SIZE,
};
pub use network::NetworkDeviceConn;
pub use usb::UsbDeviceConn;

pub enum DeviceConn {
    Network(NetworkDeviceConn),
    Usb(Arc<UsbDeviceConn>),
}

pub enum DeviceConnPacket {
    Usb(UsbDevicePacket),
    Network(Bytes),
}

impl DeviceConnPacket {
    pub fn payload(&self) -> Bytes {
        match self {
            Self::Usb(packet) => packet.payload.encode(),
            Self::Network(payload) => payload.clone(),
        }
    }
}

impl DeviceConn {
    pub const fn connection_type(&self) -> ConectionType {
        match self {
            Self::Usb(_) => ConectionType::Usb,
            Self::Network(_) => ConectionType::Network,
        }
    }
    pub const fn as_network(&mut self) -> Option<&mut NetworkDeviceConn> {
        if let Self::Network(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    pub fn as_usb(&self) -> Option<&UsbDeviceConn> {
        if let Self::Usb(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    pub fn device_id(&self) -> u64 {
        match self {
            Self::Usb(dev) => dev.device.core.id,
            Self::Network(dev) => dev.device_id,
        }
    }

    pub fn dst_port(&self) -> u16 {
        match self {
            Self::Usb(dev) => dev.destination_port,
            Self::Network(dev) => dev.destination_port,
        }
    }

    pub fn sendable_bytes(&self) -> usize {
        match self {
            Self::Usb(dev) => dev.get_sendable_bytes(),
            Self::Network(_) =>
            /*TODO: uhh */
            {
                MAX_PACKET_SIZE
            }
        }
    }

    pub async fn close(&self) -> Result<(), RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.close().await,
            Self::Network(_) => Ok(()),
        }
    }

    pub async fn send_bytes(&self, value: Bytes) -> Result<(), RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.send_bytes(value).await,
            Self::Network(dev) => Ok(dev.write(value).await?),
        }
    }

    pub async fn send_plist(&self, value: plist::Value) -> Result<(), RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.send_plist(value).await,
            Self::Network(_) => todo!(),
        }
    }

    pub async fn recv(&self) -> Result<DeviceConnPacket, RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.recv().await.map(DeviceConnPacket::Usb),
            Self::Network(dev) => dev.read().await.map(DeviceConnPacket::Network),
        }
    }

    pub async fn wait_shutdown(&self) {
        match self {
            Self::Usb(dev) => dev.wait_shutdown().await,
            Self::Network(dev) => dev.wait_shutdown().await,
        }
    }
}
