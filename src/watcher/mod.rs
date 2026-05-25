use std::{
    collections::HashMap,
    sync::{LazyLock, atomic::AtomicU64},
};

use dashmap::DashMap;

use tokio::sync::{OnceCell, broadcast};

mod network;
mod usb;

use crate::{
    device::{ConectionType, Device},
    error::RusbmuxError,
};
pub use network::watch_network;
pub(crate) use network::watch_network_daemon;

pub use usb::watch_usb;
pub(crate) use usb::watch_usb_daemon;

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

#[derive(Debug)]
pub enum DeviceWatchEvent {
    Connected(Device),
    Disconnected(u64),
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
pub async fn remove_device(id: u64) -> Result<Device, RusbmuxError> {
    let (_, device) = CONNECTED_DEVICES
        .remove(&id)
        .ok_or(RusbmuxError::DeviceNotFound(id))?;
    device.shutdown().await?;

    // if the removed device is a usb, and there's a network device connected with the same
    // serial number, it would notify the apps (whoever doing a `Listen`)
    // that the network device is now connected
    //
    // or if the removed device is a network, and there's NO usb device connected with the same
    // serial number, it would notify the apps with a detached event
    // if otherwise there's a usb device connected, it would skip, because it's already detached
    //
    // this is to dedup and expose only one device (either usb or network, not both, while also
    // prefering usb over network)
    match device.connection_type() {
        // the removed device is a usb, and there's a network device with the same serial number
        ConectionType::Usb
            if let Some(ndev) = CONNECTED_DEVICES.iter().find(|dev| {
                dev.as_network()
                    .is_some_and(|_| dev.serial_number() == device.serial_number())
            }) =>
        {
            let _ = get_hotplug_event_tx()
                .await
                .send(DeviceEvent::Attached { id: ndev.id() });
        }

        // the network device is also connected as usb, so skip sending the detached event
        //
        // because the network device is already detached from the listener (dedup purposes)
        ConectionType::Network
            if CONNECTED_DEVICES.iter().any(|dev| {
                dev.as_usb()
                    .is_some_and(|_| dev.serial_number() == device.serial_number())
            }) => {}

        // the usb disconnection event would be sent from the usb watcher
        ConectionType::Usb => {}

        // the network device is not also connected as usb
        //
        // this is a duplication from the network watcher, but sometimes mdns doesn't broadcast a
        // removal, so this will be fired if we don't get a heartbeat response from the device
        // if it did broadcast a removal, then function will return with device not found, because
        // the network watcher already removed it
        ConectionType::Network => {
            let _ = get_hotplug_event_tx()
                .await
                .send(DeviceEvent::Detached { id: device.id() });
        }
    }

    Ok(device)
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
