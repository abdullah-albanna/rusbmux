use crate::{
    AsyncWriting,
    error::RusbmuxError,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
    watcher::CONNECTED_DEVICES,
};

use tokio::io::AsyncWriteExt;
use tracing::{debug, error};

pub async fn devices_plist() -> Result<plist::Value, RusbmuxError> {
    let mut devices_plist = Vec::with_capacity(CONNECTED_DEVICES.len());

    for device in &*CONNECTED_DEVICES {
        devices_plist.push(device.create_device_attached()?);
    }

    debug!(
        "Created device list plist with {} device/s",
        devices_plist.len()
    );

    let res = plist_macro::plist!({
        "DeviceList": devices_plist
    });

    debug!("{}", plist_macro::pretty_print_plist(&res));

    Ok(res)
}

pub async fn handle_device_list(
    writer: &mut impl AsyncWriting,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let devices_plist = devices_plist().await?;

    let devices_xml = plist_macro::plist_value_to_xml_bytes(&devices_plist);

    let usbmux_packet = UsbMuxPacket::encode_from(
        devices_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );
    writer
        .write_all(&usbmux_packet)
        .await
        .inspect_err(|e| error!(tag, err = ?e, "Failed to send device list packet"))?;
    writer
        .flush()
        .await
        .inspect_err(|e| error!(tag, err = ?e, "Failed to flush device list packet"))?;

    debug!(tag, "Device list packet sent");

    Ok(())
}
