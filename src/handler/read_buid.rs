use std::path::Path;

use crate::{
    AsyncWriting,
    error::{MissingFields, RusbmuxError},
    handler::{LOCKDOWN_PATH, ResultCode, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_read_buid(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    let tag = usbmux_packet.header.tag;

    let path = format!("{LOCKDOWN_PATH}/SystemConfiguration.plist");
    let path = Path::new(&path);

    trace!(tag, "Reading SystemConfiguration.plist");

    if !path.exists() {
        let id = uuid::Uuid::new_v4().to_string().to_uppercase();
        debug!(tag, "SystemConfiguration.plist is missing, creating one");

        let sbuid = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
            "SystemBUID": id
        }));

        if let Err(e) = tokio::fs::write(&path, sbuid).await {
            error!(tag, err = ?e, "Failed to write a new SystemConfiguration.plist");
            let _ = send_result(writer, ResultCode::BadDeviceOrNoSuchFile, tag).await;
            return Ok(());
        }
    }

    let system_config = plist::from_file::<_, plist::Value>(&path)
        .inspect_err(|e| error!( tag,  err = ?e, "Failed to read SystemConfiguration.plist"))?;
    let buid = system_config
        .as_dictionary()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a dictionary plist payload".to_string(),
        ))?
        .get("SystemBUID")
        .ok_or(RusbmuxError::ValueNotFound(MissingFields::SystemBUID))?
        .as_string()
        .ok_or(RusbmuxError::InvalidData("SystemBUID is not a string"))?;

    trace!(tag, buid, "Extracted SystemBUID");

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

    writer.write_all(&usbmux_packet).await.inspect_err(|e| {
        if !crate::utils::is_disconnect_io(e) {
            error!(tag, err = ?e, "Failed to write BUID response")
        }
    })?;

    debug!(tag, "BUID response sent");

    Ok(())
}
