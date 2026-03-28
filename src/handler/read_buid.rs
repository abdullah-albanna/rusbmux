use crate::{
    AsyncWriting,
    handler::CONFIG_PATH,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_read_buid(writer: &mut impl AsyncWriting, usbmux_packet: &UsbMuxPacket) {
    let tag = usbmux_packet.header.tag;

    let path = format!("{CONFIG_PATH}/lockdown/SystemConfiguration.plist");

    trace!(tag, "Reading system configuration plist");

    let system_config = match plist::from_file::<_, plist::Value>(&path) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!( tag, path, err = ?e, "Failed to read SystemConfiguration.plist");
            return;
        }
    };

    let buid = match system_config
        .as_dictionary()
        .and_then(|d| d.get("SystemBUID"))
        .and_then(|v| v.as_string())
    {
        Some(buid) => buid,
        None => {
            error!(tag, "SystemBUID missing or invalid in plist");
            return;
        }
    };

    debug!(tag, buid, "Extracted SystemBUID");

    let response_plist = plist_macro::plist!({
        "BUID": buid
    });

    let usbmux_packet = UsbMuxPacket::encode_from(
        plist_macro::plist_value_to_xml_bytes(&response_plist),
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        usbmux_packet.header.tag,
    );

    trace!(tag, "Sending BUID response");

    if let Err(e) = writer.write_all(&usbmux_packet).await {
        error!(tag, err = ?e, "Failed to write BUID response");
        return;
    }

    if let Err(e) = writer.flush().await {
        error!(tag, err = ?e, "Failed to flush BUID response");
        return;
    }

    trace!(tag, "BUID response sent successfully");
}
