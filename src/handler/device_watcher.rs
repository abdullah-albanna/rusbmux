use std::collections::HashMap;

use tokio::sync::OnceCell;

use futures_lite::StreamExt;
use nusb::{Speed, hotplug::HotplugEvent};
use tokio::sync::{RwLock, broadcast};

use crate::{usb::APPLE_VID, utils::nusb_speed_to_number};

pub static EVENT_TX: OnceCell<broadcast::Sender<DeviceEvent>> = OnceCell::const_new();
pub static DEVICES: RwLock<Vec<(nusb::DeviceInfo, u32)>> = RwLock::const_new(vec![]);

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

pub async fn device_watcher() {
    let event_tx = EVENT_TX
        .get_or_init(|| async move { broadcast::channel::<DeviceEvent>(32).0 })
        .await;

    let mut devices = nusb::watch_devices().unwrap().filter_map(|e| {
        // don't include the connected event if it's not an apple devices
        if matches!(&e, HotplugEvent::Connected(dev) if dev.vendor_id() != APPLE_VID) {
            return None;
        }

        Some(e)
    });

    let mut device_id_counter = 0;

    let mut device_map = HashMap::new();

    let current_connected_devices = crate::usb::get_apple_device().await.collect::<Vec<_>>();

    if !current_connected_devices.is_empty() {
        let mut global_devices = DEVICES.write().await;
        for device in current_connected_devices {
            device_map
                .entry(device.id())
                .insert_entry(device_id_counter);

            global_devices.push((device, device_id_counter));
            device_id_counter += 1;
        }
    }

    while let Some(event) = devices.next().await {
        // no one is listening
        if event_tx.receiver_count() < 1 {
            continue;
        }

        println!("new event: {event:#?}");

        match event {
            HotplugEvent::Connected(device) => {
                device_map
                    .entry(device.id())
                    .insert_entry(device_id_counter);

                let speed = nusb_speed_to_number(device.speed().unwrap_or(Speed::Low));

                if let Err(e) = event_tx.send(DeviceEvent::Attached {
                    serial_number: device.serial_number().unwrap_or_default().to_string(),
                    id: device_id_counter,
                    speed,
                    product_id: device.product_id(),
                    device_address: device.device_address(),
                }) {
                    eprintln!("looks like no one is listening, error: {e}")
                }

                let mut global_devices = DEVICES.write().await;
                global_devices.push((device, device_id_counter));

                device_id_counter += 1;
            }
            HotplugEvent::Disconnected(id) => {
                if let Some(device_id) = device_map.remove(&id) {
                    let mut global_devices = DEVICES.write().await;
                    let device_idx = global_devices
                        .iter()
                        .position(|dev| dev.0.id() == id)
                        .expect("unable to get the position of the about to be removed device");
                    global_devices.remove(device_idx);

                    if let Err(e) = event_tx.send(DeviceEvent::Detached { id: device_id }) {
                        eprintln!("looks like no one is listening, error: {e}")
                    }
                }
            }
        }
    }
}
