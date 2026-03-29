use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::CONFIG_PATH,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_read_buid(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    let tag = usbmux_packet.header.tag;

    let path = format!("{CONFIG_PATH}/lockdown/SystemConfiguration.plist");

    trace!(tag, "Reading system configuration plist");

    let system_config = plist::from_file::<_, plist::Value>(&path).inspect_err(
        |e| error!( tag, path, err = ?e, "Failed to read SystemConfiguration.plist"),
    )?;

    let buid = system_config
        .as_dictionary()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a dictionary plist payload".to_string(),
        ))?
        .get("SystemBUID")
        .ok_or(RusbmuxError::ValueNotFound("SystemBUID"))?
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

    writer
        .write_all(&usbmux_packet)
        .await
        .inspect_err(|e| error!(tag, err = ?e, "Failed to write BUID response"))?;

    writer
        .flush()
        .await
        .inspect_err(|e| error!(tag, err = ?e, "Failed to flush BUID response"))?;

    debug!(tag, "BUID response sent");

    Ok(())
}
