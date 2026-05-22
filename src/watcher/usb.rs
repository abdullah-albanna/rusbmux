use futures_lite::StreamExt;
use nusb::hotplug::HotplugEvent;

use crate::{device::Device, usb::APPLE_VID};

use super::{CONNECTED_DEVICES, DeviceEvent};
use tracing::{error, trace};

pub async fn device_watcher() {
    let hotplug_event_tx = super::get_hotplug_event_tx().await;

    let mut devices_hotplug = nusb::watch_devices()
        .unwrap_or_else(|e| {
            error!(e = ?e, "Failed to create a device hotplug");
            std::process::exit(-1);
        })
        .filter_map(|e| {
            // don't include the connected event if it's not an apple devices
            if matches!(&e, HotplugEvent::Connected(dev) if dev.vendor_id() != APPLE_VID) {
                return None;
            }

            Some(e)
        });

    let mut devices_id_map = std::collections::HashMap::new();

    if let Err(e) = super::push_currently_connected_devices(&mut devices_id_map).await {
        error!(e = ?e, "Failed to store the currently connected devices");
    }

    while let Some(event) = devices_hotplug.next().await {
        trace!("{event:#?}");

        match event {
            HotplugEvent::Connected(device_info) => {
                let id = super::take_new_id();
                devices_id_map.insert(device_info.id(), id);

                match Device::new_usb(device_info, id).await {
                    Ok(device) => CONNECTED_DEVICES.insert(id, device),
                    Err(e) => {
                        error!(e = ?e, "Failed to create a new device");
                        continue;
                    }
                };

                let _ = hotplug_event_tx.send(DeviceEvent::Attached { id });
            }
            HotplugEvent::Disconnected(device_id) => {
                // remove from both the global devices, and so as the id's map
                if let Some(id) = devices_id_map.remove(&device_id) {
                    if let Err(e) = super::remove_device(id).await {
                        error!(e = ?e, "Failed to remove disconnected device");
                    }

                    let _ = hotplug_event_tx.send(DeviceEvent::Detached { id });
                }
            }
        }
    }
}
