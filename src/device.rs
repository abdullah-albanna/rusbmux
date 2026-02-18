use std::{
    collections::HashMap,
    sync::atomic::{AtomicU16, Ordering},
};

use futures_lite::StreamExt;
use nusb::{Speed, hotplug::HotplugEvent};
use tokio::sync::{OnceCell, RwLock, broadcast};

use crate::{parser::device_mux::DeviceMuxVersion, usb::APPLE_VID, utils::nusb_speed_to_number};

/// a channel used for hotplug events, once a device is connected it get broadcasted to all it's
/// subscribers
///
/// it only sends basic information about the device, the device it self is stored in
/// `CONNECTED_DEVICES`
pub static HOTPLUG_EVENT_TX: OnceCell<broadcast::Sender<DeviceEvent>> = OnceCell::const_new();

/// has the currently connected devices with it's corresponding idevice id
///
/// devices are pushed to it whenever a device is connected, and removed once the device is removed
pub static CONNECTED_DEVICES: RwLock<Vec<Device>> = RwLock::const_new(vec![]);

pub static SOURCE_PORT_COUNTER: AtomicU16 = AtomicU16::new(1);

pub fn get_next_source_port() -> u16 {
    SOURCE_PORT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

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

#[derive(Clone, Debug)]
pub struct Device {
    pub info: nusb::DeviceInfo,
    pub id: u32,

    pub conn: Option<DeviceMuxConn>,
}

impl Device {
    pub fn new(device_info: nusb::DeviceInfo, id: u32) -> Self {
        Self {
            info: device_info,
            id,
            conn: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DeviceMuxConn {
    /// what the device speaks in
    pub ver: DeviceMuxVersion,

    pub sent_seq: u16,
    pub received_seq: u16,

    pub sent_bytes: u32,
    pub received_bytes: u32,

    pub source_port: u16,
    pub destination_port: u16,
}

impl DeviceMuxConn {
    pub fn new(ver: DeviceMuxVersion, destination_port: u16) -> Self {
        Self {
            ver,
            sent_seq: 0,
            received_seq: 0,
            sent_bytes: 0,
            received_bytes: 0,
            source_port: get_next_source_port(),
            destination_port,
        }
    }
}

/// get the currently connected devices and push them to the global `CONNECTED_DEVICES` with it's device
/// id
///
/// this is necessary because the hotplug event doesn't give back currently connected devices,
/// only fresh devices fresh
pub async fn push_currently_connected_devices(
    devices_id_map: &mut HashMap<nusb::DeviceId, u32>,
    device_id_counter: &mut u32,
) {
    let current_connected_devices = crate::usb::get_apple_device().await.collect::<Vec<_>>();

    if !current_connected_devices.is_empty() {
        let mut global_devices = CONNECTED_DEVICES.write().await;
        for device_info in current_connected_devices {
            devices_id_map.insert(device_info.id(), *device_id_counter);

            global_devices.push(Device::new(device_info, *device_id_counter));
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

    let mut device_id_counter = 0;

    let mut devices_id_map = HashMap::new();

    push_currently_connected_devices(&mut devices_id_map, &mut device_id_counter).await;

    while let Some(event) = devices_hotplug.next().await {
        // no one is listening
        if hotplug_event_tx.receiver_count() < 1 {
            continue;
        }

        println!("new event: {event:#?}");

        match event {
            HotplugEvent::Connected(device_info) => {
                devices_id_map.insert(device_info.id(), device_id_counter);

                let speed = nusb_speed_to_number(device_info.speed().unwrap_or(Speed::Low));

                if let Err(e) = hotplug_event_tx.send(DeviceEvent::Attached {
                    serial_number: device_info.serial_number().unwrap_or_default().to_string(),
                    id: device_id_counter,
                    speed,
                    product_id: device_info.product_id(),
                    device_address: device_info.device_address(),
                }) {
                    eprintln!("looks like no one is listening, error: {e}")
                }

                let mut global_devices = CONNECTED_DEVICES.write().await;
                global_devices.push(Device::new(device_info, device_id_counter));

                device_id_counter += 1;
            }
            HotplugEvent::Disconnected(device_id) => {
                // remove from both the global devices, and so as the id's map
                if let Some(id) = devices_id_map.remove(&device_id) {
                    remove_device(device_id).await;

                    if let Err(e) = hotplug_event_tx.send(DeviceEvent::Detached { id }) {
                        eprintln!("looks like no one is listening, error: {e}")
                    }
                }
            }
        }
    }
}
