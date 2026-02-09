use futures_lite::StreamExt;
use nusb::{Speed, hotplug::HotplugEvent};

use crate::{DeviceEvent, usb::APPLE_VID, utils::nusb_speed_to_number};

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
