use std::{collections::HashMap, net::IpAddr, path::Path};

use futures_lite::Stream;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use idevice::pairing_file::PairingFile;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use sha2::{Sha256, Sha512};
use tracing::{debug, error, info, warn};

use base64::{Engine as _, engine::general_purpose::STANDARD as Base64};

use crate::{
    device::Device,
    error::RusbmuxError,
    handler::LOCKDOWN_PATH,
    usb_backend::take_new_id,
    watcher::{DeviceEvent, get_hotplug_event_tx},
};

#[derive(Debug)]
pub enum NetworkEvent {
    Connected(Device),
    Disconnected(u64),
}

use super::CONNECTED_DEVICES;

pub const SERVICE_TYPE: &str = "_apple-mobdev2._tcp.local.";

pub async fn watch_network_daemon() {
    let mdns = ServiceDaemon::new().expect("Failed to create daemon");

    let receiver = mdns.browse(SERVICE_TYPE).expect("Failed to browse");

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

                debug!(mac_address, "Removing network device");

                let hotplug = get_hotplug_event_tx().await;

                let Some(id) = CONNECTED_DEVICES
                    .iter()
                    .find(|dev| {
                        dev.as_network()
                            .is_some_and(|ndev| ndev.mac_address == mac_address)
                    })
                    .map(|dev| dev.id())
                else {
                    continue;
                };

                let (_, device) = CONNECTED_DEVICES.remove(&id).expect("this shouldn't fail");
                let _ = device.shutdown().await;

                // only send the detached event if:
                //  1. it's the network device that we're trying to remove
                //  2. that device is not connected as usb (because the network device would be
                // detached right now for dedup purposes)
                if !CONNECTED_DEVICES.iter().any(|dev| {
                    dev.as_usb()
                        .is_some_and(|_| dev.serial_number() == device.serial_number())
                }) {
                    let _ = hotplug.send(DeviceEvent::Detached { id });
                }
            }
            _ => {}
        }
    }
}

pub async fn watch_network() -> impl Stream<Item = Result<NetworkEvent, RusbmuxError>> {
    async_stream::try_stream! {
        let mdns = ServiceDaemon::new().expect("Failed to create daemon");

        let receiver = mdns.browse(SERVICE_TYPE).expect("Failed to browse");

        let mut devices_id_map = HashMap::new();
        while let Ok(event) = receiver.recv_async().await {
            match event {
                ServiceEvent::ServiceResolved(rs) => {
                    let Some(rd) = resolve_service(rs) else {
                        continue;
                    };

                    let id = take_new_id();

                    let device = Device::new_network(
                        id,
                        rd.addr,
                        Some(rd.scope_id),
                        rd.mac_address.clone(),
                        rd.service_name,
                        rd.udid
                    ).await?;

                    devices_id_map.insert(rd.mac_address, id);

                    yield NetworkEvent::Connected(device);

                }
                ServiceEvent::ServiceRemoved(_, name) => {
                    let Some(mac_address) = name.split('@').next() else {
                        debug!(
                            service_name = name,
                            "`@` was not found in the removed service name"
                        );
                        continue;
                    };

                    if let Some(id) = devices_id_map.get(mac_address) {
                        yield NetworkEvent::Disconnected(*id)
                    };
                }
                _ => {}
            }
        }
    }
}

struct ResolvedDevice {
    addr: IpAddr,
    scope_id: u32,
    mac_address: String,
    service_name: String,
    udid: String,
}

fn resolve_service(rs: Box<ResolvedService>) -> Option<ResolvedDevice> {
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
        return None;
    };

    // iOS 26.4+: match by Bonjour TXT record (identifier + authTag HMACs).
    let udid = if let Some(ident) = identifier
        && !auth_tags.is_empty()
    {
        let Some(udid) = find_udid_from_txt(ident, &auth_tags) else {
            warn!("The device doesn't have a pairing file saved, skipping");
            return None;
        };

        udid
    } else {
        // iOS < 26.4 fallback: parse MAC out of the instance name (`<MAC>@<id>.…`).
        let Some(udid) = get_udid_from_mac_addr(mac_address) else {
            warn!(
                mac_address,
                "The device doesn't have a pairing file saved, skipping"
            );
            return None;
        };
        udid
    };

    Some(ResolvedDevice {
        addr,
        scope_id,
        mac_address: mac_address.to_string(),
        service_name: rs.fullname,
        udid,
    })
}

async fn network_device_add(rs: Box<ResolvedService>) {
    let Some(rd) = resolve_service(rs) else {
        return;
    };

    // if the device broadcasted twice, and the first is still connecting to the heartbeat, this
    // would get the device connect twice
    //
    // TODO: check on the resolved service it self
    if CONNECTED_DEVICES.iter().any(|dev| {
        dev.as_network()
            .is_some_and(|ndev| ndev.serial_number == rd.udid)
    }) {
        debug!(serial_number = &rd.udid, "Device already added, skipping");
        return;
    }

    let device = match Device::new_network(
        take_new_id(),
        rd.addr,
        Some(rd.scope_id),
        rd.mac_address,
        rd.service_name,
        rd.udid.clone(),
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            error!(udid = rd.udid, error = ?e, "Coudn't create a new network device");
            return;
        }
    };

    let id = device.id();
    CONNECTED_DEVICES.insert(id, device);

    // prefer usb devices over network devices for the same udid
    //
    // skip hotplug notifications when the device is already connected via usb
    //
    // it would get notified only if the usb is disconnected
    let has_usb_connection = CONNECTED_DEVICES.iter().any(|device| {
        device
            .as_usb()
            .is_some_and(|_| device.serial_number() == rd.udid)
    });

    if !has_usb_connection {
        let _ = super::get_hotplug_event_tx()
            .await
            .send(super::DeviceEvent::Attached { id });
    }
}

fn find_udid_from_txt(identifier: &[u8], auth_tags: &[&[u8]]) -> Option<String> {
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
    let decoded = Base64.decode(trimmed).ok()?;
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

            let Ok(pf) = PairingFile::read_from_file(path) else {
                return None;
            };

            Some((udid, pf))
        })
        .collect()
}

fn get_udid_from_mac_addr(mac_addr: &str) -> Option<String> {
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
