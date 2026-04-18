pub mod core;
pub mod network;
pub mod packet_router;
pub mod usb;
use std::{net::IpAddr, sync::Arc};

use network::NetworkDevice;
use usb::UsbDevice;

use crate::{conn::DeviceConn, error::RusbmuxError};

pub enum Device {
    Network(NetworkDevice),
    Usb(Arc<UsbDevice>),
}

#[derive(Debug)]
pub enum ConectionType {
    Usb,
    Network,
}

impl Device {
    #[must_use]
    pub const fn connection_type(&self) -> ConectionType {
        match self {
            Self::Usb(_) => ConectionType::Usb,
            Self::Network(_) => ConectionType::Network,
        }
    }

    #[must_use]
    pub fn id(&self) -> u64 {
        match self {
            Self::Network(dev) => dev.core.id,
            Self::Usb(dev) => dev.core.id,
        }
    }

    pub async fn new_usb(info: nusb::DeviceInfo, id: u64) -> Result<Self, RusbmuxError> {
        Ok(Self::Usb(UsbDevice::new(info, id).await?))
    }

    pub async fn new_network(
        id: u64,
        addr: IpAddr,
        scope_id: Option<u32>,
        mac_address: String,
        service_name: String,
        serial_number: String,
    ) -> Result<Self, RusbmuxError> {
        Ok(Self::Network(
            NetworkDevice::new(id, addr, scope_id, mac_address, service_name, serial_number)
                .await?,
        ))
    }

    pub async fn shutdown(&self) -> Result<(), RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.shutdown().await,
            Self::Network(dev) => dev.shutdown(),
        }
    }

    pub async fn connect(&self, port: u16) -> Result<DeviceConn, RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.connect(port).await.map(DeviceConn::Usb),
            Self::Network(dev) => dev.connect(port).await.map(DeviceConn::Network),
        }
    }

    #[must_use]
    pub const fn as_network(&self) -> Option<&NetworkDevice> {
        if let Self::Network(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_usb(&self) -> Option<&Arc<UsbDevice>> {
        if let Self::Usb(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    pub fn create_device_attached(&self) -> Result<plist::Value, RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.create_device_attached(),
            Self::Network(dev) => dev.create_device_attached(),
        }
    }
}
