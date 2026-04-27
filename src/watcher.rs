use std::{
    collections::HashMap,
    net::IpAddr,
    path::Path,
    sync::{LazyLock, atomic::AtomicU64},
};

use dashmap::DashMap;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use nusb::hotplug::HotplugEvent;
use tokio::sync::{OnceCell, broadcast};
use tracing::{debug, error, trace, warn};

use crate::{device::Device, error::RusbmuxError, handler::CONFIG_PATH, usb::APPLE_VID};
use futures_lite::StreamExt;

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

pub async fn device_watcher() {
    let hotplug_event_tx = get_hotplug_event_tx().await;

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

    let mut devices_id_map = HashMap::new();

    if let Err(e) = push_currently_connected_devices(&mut devices_id_map).await {
        error!(e = ?e, "Failed to store the currently connected devices");
    }

    while let Some(event) = devices_hotplug.next().await {
        trace!("{event:#?}");

        match event {
            HotplugEvent::Connected(device_info) => {
                let id = take_new_id();
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
                    if let Err(e) = remove_device(id).await {
                        error!(e = ?e, "Failed to remove disconnected device");
                    }

                    let _ = hotplug_event_tx.send(DeviceEvent::Detached { id });
                }
            }
        }
    }
}

pub async fn network_watcher() {
    let mdns = ServiceDaemon::new().expect("Failed to create daemon");

    let service_type = "_apple-mobdev2._tcp.local.";
    let receiver = mdns.browse(service_type).expect("Failed to browse");

    while let Ok(event) = receiver.recv_async().await {
        match event {
            ServiceEvent::ServiceResolved(rs) => {
                tokio::spawn(network_device_add(rs));
            }
            ServiceEvent::ServiceRemoved(_, name) => {
                let Some(mac_address) = name.split('@').next() else {
                    debug!(
                        service_name = name,
                        "`@` was not found in the removed service name"
                    );
                    continue;
                };

                CONNECTED_DEVICES.retain(|_, dev| {
                    dev.as_network()
                        .is_none_or(|ndev| ndev.mac_address == mac_address)
                });
            }
            _ => {}
        }
    }
}

pub async fn network_device_add(rs: Box<ResolvedService>) {
    debug!("Discovered network device via mDNS: {rs:#?}");
    let addresses = rs.addresses;

    let (addr, scope_id) = if addresses.iter().any(mdns_sd::ScopedIp::is_ipv6) {
        let mdns_sd::ScopedIp::V6(addr) = addresses
            .into_iter()
            .find(mdns_sd::ScopedIp::is_ipv6)
            .unwrap()
        else {
            unreachable!()
        };

        (IpAddr::V6(*addr.addr()), addr.scope_id().index)
    } else {
        let mdns_sd::ScopedIp::V4(addr) = addresses
            .into_iter()
            .find(mdns_sd::ScopedIp::is_ipv4)
            .unwrap()
        else {
            unreachable!()
        };

        (
            IpAddr::V4(*addr.addr()),
            addr.interface_ids().first().map_or(0, |i| i.index),
        )
    };

    let name = rs.fullname;

    let Some(mac_address) = name.split('@').next() else {
        warn!(
            service_name = name,
            "`@` was not found in the service name, skipping"
        );
        return;
    };

    let Some(udid) = get_udid_from_mac_addr(mac_address) else {
        warn!(
            mac_address,
            "The device doesn't have a pairing file saved, skipping"
        );
        return;
    };

    if CONNECTED_DEVICES.iter().any(|dev| {
        dev.as_network()
            .is_some_and(|ndev| ndev.mac_address == mac_address)
    }) {
        debug!(mac_address, "Device already added, skipping");
        return;
    }

    let device = match Device::new_network(
        take_new_id(),
        addr,
        Some(scope_id),
        mac_address.to_string(),
        name,
        udid.clone(),
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            error!(udid, error = ?e, "Coudn't create a new network device");
            return;
        }
    };

    let id = device.id();
    CONNECTED_DEVICES.insert(id, device);

    let _ = get_hotplug_event_tx()
        .await
        .send(DeviceEvent::Attached { id });
}

#[must_use]
pub fn get_udid_from_mac_addr(mac_addr: &str) -> Option<String> {
    for path in Path::new(&format!("{CONFIG_PATH}/lockdown/"))
        .read_dir()
        .unwrap()
        .flatten()
    {
        let file_path = path.path();
        if file_path
            .extension()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default()
            == "plist"
        {
            let pair_record: plist::Dictionary = plist::from_file(&file_path).ok()?;
            let Some(pair_record_mac_addr) = pair_record.get("WiFiMACAddress") else {
                continue;
            };

            if pair_record_mac_addr.as_string()? == mac_addr {
                // /var/lib/lockdown/67676767-6767676767676767.plist
                //
                //                  |------------------------|
                //                          takes this

                return Some(file_path.file_stem()?.to_str()?.to_string());
            }
        }
    }

    None
}
