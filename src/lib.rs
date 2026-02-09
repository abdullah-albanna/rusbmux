use futures_lite::StreamExt;
use nusb::{Speed, hotplug::HotplugEvent};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::usb::APPLE_VID;

pub mod handler;
pub mod parser;
pub mod usb;

pub trait ReadWrite: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> ReadWrite for T {}

pub trait AsyncReading: AsyncRead + Unpin + Send + Sync {}
impl<T: AsyncRead + Unpin + Send + Sync> AsyncReading for T {}

pub trait AsyncWriting: AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncWrite + Unpin + Send + Sync> AsyncWriting for T {}

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Attached {
        serial_number: String,
        id: u32,
        speed: u32,
        product_id: u16,
        device_address: u8,
    },
    Detached {
        id: u32,
    },
}

pub(crate) fn nusb_speed_to_number(speed: Speed) -> u32 {
    match speed {
        Speed::Low => 1,
        Speed::Full => 12,
        Speed::High => 480,
        Speed::Super => 5000,
        Speed::SuperPlus => 10000,
        unknown => panic!("unknown device speed: {unknown:?}"),
    }
}
pub async fn device_watcher(event_tx: tokio::sync::broadcast::Sender<DeviceEvent>) {
    let mut devices = nusb::watch_devices().unwrap().filter_map(|e| {
        // don't include the connected event if it's not an apple devices
        if matches!(&e, HotplugEvent::Connected(dev) if dev.vendor_id() != APPLE_VID) {
            return None;
        }

        Some(e)
    });

    while let Some(event) = devices.next().await {
        // no one is listening
        if event_tx.receiver_count() < 1 {
            continue;
        }

        // TODO: the device id should be unique to each device
        // the id that it connected with should be the one it disconnects with too
        match event {
            HotplugEvent::Connected(device) => {
                let speed = nusb_speed_to_number(device.speed().unwrap_or(Speed::Low));

                if let Err(e) = event_tx.send(DeviceEvent::Attached {
                    serial_number: device.serial_number().unwrap_or_default().to_string(),
                    id: 0,
                    speed,
                    product_id: device.product_id(),
                    device_address: device.device_address(),
                }) {
                    eprintln!("looks like no one is listening, error: {e}")
                }
            }
            HotplugEvent::Disconnected(_) => {
                if let Err(e) = event_tx.send(DeviceEvent::Detached { id: 0 }) {
                    eprintln!("looks like no one is listening, error: {e}")
                }
            }
        }
    }
}
