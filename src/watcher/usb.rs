use futures_lite::StreamExt;

use crate::{
    device::Device,
    usb_backend::{self, APPLE_VID, UsbBackend},
};

use super::{CONNECTED_DEVICES, DeviceEvent};
use tracing::{error, trace};

pub async fn watch_usb_daemon(backend: impl UsbBackend) {
    let hotplug_event_tx = super::get_hotplug_event_tx().await;

    let mut devices_hotplug = backend
        .watch_devices()
        .await
        .unwrap_or_else(|e| {
            error!(e = ?e, "Failed to create a device hotplug");
            std::process::exit(-1);
        })
        .filter_map(|e| {
            // don't include the connected event if it's not an apple devices
            if matches!(&e, Ok(usb_backend::Event::Connected(dev, _)) if dev.vendor_id() != APPLE_VID)
            {
                return None;
            }

            Some(e)
        });

    while let Some(event) = devices_hotplug.next().await {
        trace!("{event:#?}");

        match event {
            Ok(usb_backend::Event::Connected(device_info, id)) => {
                let device = Device::new_usb(device_info, id).await;
                match device {
                    Ok(device) => {
                        if let Some(ndev) = CONNECTED_DEVICES.iter().find(|dev| {
                            dev.as_network()
                                .is_some_and(|_| dev.serial_number() == device.serial_number())
                        }) {
                            let _ = hotplug_event_tx.send(DeviceEvent::Detached { id: ndev.id() });
                        }

                        CONNECTED_DEVICES.insert(id, device);
                    }
                    Err(e) => {
                        error!(e = ?e, "Failed to create a new device");
                        continue;
                    }
                };

                let _ = hotplug_event_tx.send(DeviceEvent::Attached { id });
            }
            Ok(usb_backend::Event::Disconnected(id)) => {
                if let Err(e) = super::remove_device(id).await {
                    error!(e = ?e, "Failed to remove disconnected device");
                }

                // TODO: maybe we can put this into the `remove_device` function instead
                let _ = hotplug_event_tx.send(DeviceEvent::Detached { id });
            }
            Err(e) => error!(?e, "Hotplug error"),
        }
    }
}
