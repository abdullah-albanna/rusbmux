use std::{
    collections::HashMap,
    net::IpAddr,
    path::Path,
    sync::{LazyLock, atomic::AtomicU64},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use dashmap::DashMap;

use idevice::pairing_file::PairingFile;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use nusb::hotplug::HotplugEvent;
use tokio::sync::{OnceCell, broadcast};
use tracing::{debug, error, info, trace, warn};

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha512};

use crate::{device::Device, error::RusbmuxError, handler::LOCKDOWN_PATH, usb::APPLE_VID};
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

                // HACK: windows fails to open it right away unless I wait just a little, not sure
                // why
                #[cfg(target_os = "windows")]
                {
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }

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
    let addresses = rs.addresses.clone();

    // perfer ipv6 if available
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

    // iOS 26.4+: match by Bonjour TXT record (identifier + authTag HMACs).
    let identifier = rs.get_property_val("identifier").flatten();
    let auth_tags: Vec<&[u8]> = rs
        .get_properties()
        .iter()
        .filter(|p| {
            let k = p.key();
            k == "authTag" || k.starts_with("authTag#")
        })
        .filter_map(|p| p.val())
        .collect();

    let Some(mac_address) = rs.fullname.split('@').next() else {
        warn!(
            service_name = rs.fullname,
            "`@` was not found in the service name, skipping"
        );
        return;
    };

    // iOS 26.4+: match by Bonjour TXT record (identifier + authTag HMACs).
    let udid = if let Some(ident) = identifier
        && !auth_tags.is_empty()
    {
        let Some(udid) = find_udid_from_txt(ident, &auth_tags) else {
            warn!("The device doesn't have a pairing file saved, skipping");
            return;
        };

        udid
    } else {
        // iOS < 26.4 fallback: parse MAC out of the instance name (`<MAC>@<id>.…`).
        let Some(udid) = get_udid_from_mac_addr(mac_address) else {
            warn!(
                mac_address,
                "The device doesn't have a pairing file saved, skipping"
            );
            return;
        };
        udid
    };

    if CONNECTED_DEVICES.iter().any(|dev| {
        dev.as_network()
            .is_some_and(|ndev| ndev.serial_number == udid)
    }) {
        debug!(serial_number = udid, "Device already added, skipping");
        return;
    }

    let device = match Device::new_network(
        take_new_id(),
        addr,
        Some(scope_id),
        mac_address.to_string(),
        rs.fullname.clone(),
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

pub fn find_udid_from_txt(identifier: &[u8], auth_tags: &[&[u8]]) -> Option<String> {
    if auth_tags.is_empty() {
        return None;
    }

    // Decode all tags up front (they're independent of the candidate HostID).
    let decoded_tags: Vec<[u8; 8]> = auth_tags
        .iter()
        .filter_map(|t| decode_auth_tag(t))
        .collect();

    if decoded_tags.is_empty() {
        debug!("TXT record had authTag(s) but none decoded to 8 bytes");
        return None;
    }

    if let Some(udid) = match_txt(identifier, &decoded_tags) {
        return Some(udid);
    }

    None
}

/// Decode an `authTag` TXT value to its 8-byte form.
///
/// Bonjour TXT values are raw bytes; the `authTag` entries carry base64-encoded
/// 8-byte HMAC truncations. MobileDevice trims ASCII whitespace before decoding
/// (see `_EVP_DecodeBlock` site in `AMDIsTXTRecordForUDID`). Anything that
/// doesn't decode to exactly 8 bytes is rejected.
fn decode_auth_tag(raw: &[u8]) -> Option<[u8; 8]> {
    let trimmed = raw
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map(|start| {
            let end = raw
                .iter()
                .rposition(|b| !b.is_ascii_whitespace())
                .map(|i| i + 1)
                .unwrap_or(raw.len());
            &raw[start..end]
        })
        .unwrap_or(&[][..]);
    let decoded = B64.decode(trimmed).ok()?;
    decoded.as_slice().try_into().ok()
}

fn match_txt(identifier: &[u8], decoded_tags: &[[u8; 8]]) -> Option<String> {
    for (udid, PairingFile { host_id, .. }) in get_saved_pairing_files() {
        let hk = Hkdf::<Sha512>::new(None, host_id.as_bytes());
        let mut key = [0u8; 32];
        if hk.expand(&[], &mut key).is_err() {
            continue;
        }

        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).ok()?;
        mac.update(identifier);
        let tag = mac.finalize().into_bytes();
        let expected = &tag[..8];
        if decoded_tags.iter().any(|d| d == expected) {
            info!(udid, "TXT record matched UDID");
            return Some(udid);
        }
    }
    None
}

/// gets all the valid pairing files along side it's file stem (udid)
fn get_saved_pairing_files() -> Vec<(String, PairingFile)> {
    Path::new(LOCKDOWN_PATH)
        .read_dir()
        .unwrap()
        .flatten()
        .map(|di| di.path())
        .map(|path| {
            (
                path.file_stem()
                    .and_then(|fstem| fstem.to_str())
                    .map(|s| s.to_string()),
                path,
            )
        })
        .flat_map(|(fstem, path)| {
            let udid = fstem?;

            // TODO: silence it from logging
            let Ok(pf) = PairingFile::read_from_file(path) else {
                return None;
            };

            Some((udid, pf))
        })
        .collect()
}

pub fn get_udid_from_mac_addr(mac_addr: &str) -> Option<String> {
    for (
        udid,
        PairingFile {
            wifi_mac_address, ..
        },
    ) in get_saved_pairing_files()
    {
        if mac_addr == wifi_mac_address {
            return Some(udid);
        }
    }

    None
}
