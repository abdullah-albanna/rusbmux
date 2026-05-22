use std::{
    collections::HashMap,
    sync::{LazyLock, atomic::AtomicU64},
};

use dashmap::DashMap;

use tokio::sync::{OnceCell, broadcast};

mod network;
mod usb;

use crate::{device::Device, error::RusbmuxError};
pub use network::network_watcher;
pub use usb::device_watcher;

/// a channel used for hotplug events, once a device is connected it gets broadcasted to all it's
/// subscribers
///
/// it only sends the device id, the device it self is stored in
/// `CONNECTED_DEVICES`
pub static HOTPLUG_EVENT_TX: OnceCell<broadcast::Sender<DeviceEvent>> = OnceCell::const_new();

/// has the currently connected devices with it's corresponding idevice id
///
/// devices are pushed to it whenever a device is connected, and removed once the device is removed
pub static CONNECTED_DEVICES: LazyLock<DashMap<u64, Device>> = LazyLock::new(DashMap::new);

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Attached { id: u64 },
    Detached { id: u64 },
}

/// get the currently connected devices and push them to the global `CONNECTED_DEVICES` with it's device
/// id
///
/// this is necessary because the hotplug event doesn't give back currently connected devices,
/// only fresh devices
pub async fn push_currently_connected_devices(
    devices_id_map: &mut HashMap<nusb::DeviceId, u64>,
) -> Result<(), RusbmuxError> {
    let current_connected_devices = crate::usb::get_apple_device().await.collect::<Vec<_>>();

    if !current_connected_devices.is_empty() {
        for device_info in current_connected_devices {
            let device_id = take_new_id();

            devices_id_map.insert(device_info.id(), device_id);

            let device = Device::new_usb(device_info, device_id).await?;
            CONNECTED_DEVICES.insert(device_id, device);
        }
    }

    Ok(())
}

/// Removes the device from the connected devices and shut it down
pub async fn remove_device(id: u64) -> Result<(), RusbmuxError> {
    CONNECTED_DEVICES
        .remove(&id)
        .ok_or(RusbmuxError::DeviceNotFound(id))?
        .1
        .shutdown()
        .await
}

pub static DEVICE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[inline]
pub fn take_new_id() -> u64 {
    DEVICE_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[inline]
pub async fn get_hotplug_event_tx() -> &'static broadcast::Sender<DeviceEvent> {
    HOTPLUG_EVENT_TX
        .get_or_init(|| async move { broadcast::channel::<DeviceEvent>(32).0 })
        .await
}
