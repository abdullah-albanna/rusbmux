use std::time::Duration;

use crossfire::mpsc;
use futures_lite::{Stream, StreamExt};
use tokio::time::Instant;

use crate::{
    device::Device,
    error::RusbmuxError,
    usb_backend::{self, APPLE_VID, UsbBackend},
};

use super::{CONNECTED_DEVICES, DeviceEvent};
use tracing::{debug, error, trace};

pub enum UsbEvent {
    Connected((Device, u64)),
    Disconnected(u64),
}

pub fn watch_usb(backend: &impl UsbBackend) -> impl Stream<Item = Result<UsbEvent, RusbmuxError>> {
    async_stream::try_stream! {
        let mut devices_hotplug = backend
            .watch_devices()
            .await?
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
                    let opaque_id = device_info.opaque_id();
                    let device = match Device::new_usb(device_info, id).await {
                        Ok(device) => Ok(device),
                        Err(first_error) => {
                            let deadline = Instant::now() + Duration::from_secs(3);

                            loop {
                                if Instant::now() >= deadline {
                                    break Err(first_error);
                                }

                                let device_info = backend
                                    .list_devices()
                                    .await
                                    .into_iter()
                                    .find(|device| device.opaque_id() == opaque_id);

                                if let Some(device_info) = device_info
                                    && let Ok(device) = Device::new_usb(device_info, id).await
                                {
                                    break Ok(device);
                                }

                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    };

                    yield UsbEvent::Connected((device?, id));

                }
                Ok(usb_backend::Event::Disconnected(id)) => {
                    yield UsbEvent::Disconnected(id);
                }
                Err(e) => error!(?e, "Hotplug error"),
            }
        }
    }
}

pub async fn watch_usb_daemon(backend: impl UsbBackend) {
    let hotplug_event_tx = super::get_hotplug_event_tx().await;

    let usb_hotplug = watch_usb(&backend);

    tokio::pin!(usb_hotplug);

    let (disconnected_tx, disconnected_rx) = mpsc::bounded_async(32);

    loop {
        tokio::select! {
            Some(event) = usb_hotplug.next() => {
                if let Ok(UsbEvent::Connected((device, _))) = &event
                    && let Some(usb) = device.as_usb()
                {
                    usb.set_disconnected_tx(disconnected_tx.clone());
                }

                match event {
                    Ok(UsbEvent::Connected((device, id))) => {
                        if let Some(ndev) = CONNECTED_DEVICES.iter().find(|dev| {
                            dev.as_network()
                                .is_some_and(|_| dev.serial_number() == device.serial_number())
                        }) {
                            let _ = hotplug_event_tx.send(DeviceEvent::Detached { id: ndev.id() });
                        }

                        // A device may emit multiple connect events (especially during boot), and the
                        // initial connection may not receive a matching disconnect event. Remove any
                        // stale entry before registering the new connection.
                        CONNECTED_DEVICES.retain(|_, d| d.as_network().is_some() || d.serial_number() != device.serial_number());

                        // TODO: do preflight
                        CONNECTED_DEVICES.insert(id, device);

                        let _ = hotplug_event_tx.send(DeviceEvent::Attached { id });
                    }
                    Ok(UsbEvent::Disconnected(id)) => {
                        match super::remove_device(id).await {
                            Ok(_) => {}
                            Err(RusbmuxError::DeviceNotFound(_)) => {}
                            Err(e) => error!(e = ?e, "Failed to remove disconnected device"),
                        }
                    }
                    Err(e) => {
                        error!(e = ?e, "Failed to create a new device");
                        continue;
                    }
                }
            }

            // the io kind of disconnection happens only in usb
            Ok((id, opaque_id)) = disconnected_rx.recv() => {
                match super::remove_device(id).await {
                    Ok(_) => {}
                    Err(RusbmuxError::DeviceNotFound(_)) => {}
                    Err(e) => error!(e = ?e, "Failed to remove disconnected device"),
                }

                debug!("Trying to repon closed device");

                // try to bring it back
                let device_info = backend
                    .list_devices()
                    .await
                    .into_iter()
                    .find(|device| device.opaque_id() == opaque_id);

                if let Some(device_info) = device_info
                    && let Ok(device) = Device::new_usb(device_info, id).await
                {
                    device.as_usb().unwrap().set_disconnected_tx(disconnected_tx.clone());

                    CONNECTED_DEVICES.insert(id, device);

                    let _ = hotplug_event_tx.send(DeviceEvent::Attached { id });
                }
            }

        }
    }
}
