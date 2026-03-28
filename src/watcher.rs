use std::{collections::HashMap, sync::Arc};

use nusb::{Speed, hotplug::HotplugEvent};
use tokio::sync::{OnceCell, RwLock, broadcast};

use crate::{
    device::Device,
    usb::APPLE_VID,
    utils::{get_serial_number, nusb_speed_to_number},
};
use futures_lite::StreamExt;

/// a channel used for hotplug events, once a device is connected it get broadcasted to all it's
/// subscribers
///
/// it only sends basic information about the device, the device it self is stored in
/// `CONNECTED_DEVICES`
pub static HOTPLUG_EVENT_TX: OnceCell<broadcast::Sender<DeviceEvent>> = OnceCell::const_new();

/// has the currently connected devices with it's corresponding idevice id
///
/// devices are pushed to it whenever a device is connected, and removed once the device is removed
pub static CONNECTED_DEVICES: RwLock<Vec<Arc<Device>>> = RwLock::const_new(vec![]);

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Attached {
        serial_number: String,
        id: u64,
        speed: u64,
        product_id: u16,
        location_id: u32,
    },
    Detached {
        id: u64,
    },
}

/// get the currently connected devices and push them to the global `CONNECTED_DEVICES` with it's device
/// id
///
/// this is necessary because the hotplug event doesn't give back currently connected devices,
/// only fresh devices fresh
pub async fn push_currently_connected_devices(
    devices_id_map: &mut HashMap<nusb::DeviceId, u64>,
    device_id_counter: &mut u64,
) {
    let current_connected_devices = crate::usb::get_apple_device().await.collect::<Vec<_>>();

    if !current_connected_devices.is_empty() {
        let mut global_devices = CONNECTED_DEVICES.write().await;
        for device_info in current_connected_devices {
            devices_id_map.insert(device_info.id(), *device_id_counter);

            global_devices.push(Device::new(device_info, *device_id_counter).await);
            *device_id_counter += 1;
        }
    }
}

pub async fn remove_device(id: nusb::DeviceId) {
    let mut global_devices = CONNECTED_DEVICES.write().await;
    let device_idx = global_devices
        .iter()
        .position(|dev| dev.info.id() == id)
        .expect("unable to get the position of the about to be removed device");
    global_devices.remove(device_idx);
}
pub async fn device_watcher() {
    let hotplug_event_tx = HOTPLUG_EVENT_TX
        .get_or_init(|| async move { broadcast::channel::<DeviceEvent>(32).0 })
        .await;

    let mut devices_hotplug = nusb::watch_devices().unwrap().filter_map(|e| {
        // don't include the connected event if it's not an apple devices
        if matches!(&e, HotplugEvent::Connected(dev) if dev.vendor_id() != APPLE_VID) {
            return None;
        }

        Some(e)
    });

    let mut device_id_counter = 1;

    let mut devices_id_map = HashMap::new();

    push_currently_connected_devices(&mut devices_id_map, &mut device_id_counter).await;

    while let Some(event) = devices_hotplug.next().await {
        // // no one is listening
        // if hotplug_event_tx.receiver_count() < 1 {
        //     continue;
        // }

        match event {
            HotplugEvent::Connected(device_info) => {
                devices_id_map.insert(device_info.id(), device_id_counter);

                let speed = nusb_speed_to_number(device_info.speed().unwrap_or(Speed::Low));

                #[cfg(any(target_os = "linux", target_os = "android"))]
                let location_id =
                    (device_info.busnum() as u32) << 16 | device_info.device_address() as u32;

                #[cfg(target_os = "macos")]
                let location_id = device_info.location_id();

                if let Err(_) = hotplug_event_tx.send(DeviceEvent::Attached {
                    serial_number: get_serial_number(&device_info).to_string(),
                    id: device_id_counter,
                    speed,
                    product_id: device_info.product_id(),
                    location_id,
                }) {
                    // eprintln!("looks like no one is listening, error: {e}");
                }

                CONNECTED_DEVICES
                    .write()
                    .await
                    .push(Device::new(device_info, device_id_counter).await);

                device_id_counter += 1;
            }
            HotplugEvent::Disconnected(device_id) => {
                // remove from both the global devices, and so as the id's map
                if let Some(id) = devices_id_map.remove(&device_id) {
                    remove_device(device_id).await;

                    if let Err(_) = hotplug_event_tx.send(DeviceEvent::Detached { id }) {
                        // eprintln!("looks like no one is listening, error: {e}");
                    }
                }
            }
        }
    }
}
