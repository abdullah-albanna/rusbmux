use std::{net::IpAddr, path::Path};

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use idevice::pairing_file::PairingFile;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use sha2::{Sha256, Sha512};
use tracing::{debug, error, info, warn};

use base64::{Engine as _, engine::general_purpose::STANDARD as Base64};

use crate::{device::Device, handler::CONFIG_PATH};

use super::CONNECTED_DEVICES;

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
        super::take_new_id(),
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

    let _ = super::get_hotplug_event_tx()
        .await
        .send(super::DeviceEvent::Attached { id });
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
    Path::new(&format!("{CONFIG_PATH}/lockdown/"))
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
