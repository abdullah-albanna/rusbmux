pub mod core;
pub mod network;
pub mod packet_router;
pub mod usb;
use std::{borrow::Cow, net::IpAddr, sync::Arc};

use network::NetworkDevice;
use usb::UsbDevice;

use crate::{conn::DeviceConn, error::RusbmuxError, usb_backend::AnyDeviceInfo};

#[derive(Debug)]
pub enum Device {
    Network(NetworkDevice),
    Usb(Arc<UsbDevice>),
}

#[derive(Debug)]
pub enum ConnectionType {
    Usb,
    Network,
}

impl Device {
    #[must_use]
    pub const fn connection_type(&self) -> ConnectionType {
        match self {
            Self::Usb(_) => ConnectionType::Usb,
            Self::Network(_) => ConnectionType::Network,
        }
    }

    #[must_use]
    pub fn id(&self) -> u64 {
        match self {
            Self::Network(dev) => dev.core.id,
            Self::Usb(dev) => dev.core.id,
        }
    }

    pub fn serial_number(&self) -> Cow<'_, str> {
        match self {
            Self::Network(dev) => Cow::Borrowed(&dev.serial_number),
            Self::Usb(dev) => dev.info.serial_number().unwrap_or("".into()),
        }
    }

    pub async fn new_usb(info: AnyDeviceInfo, id: u64) -> Result<Self, RusbmuxError> {
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
            Self::Usb(dev) => dev.shutdown().await?,
            Self::Network(dev) => dev.shutdown(),
        }

        Ok(())
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
