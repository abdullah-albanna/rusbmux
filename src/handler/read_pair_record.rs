use crate::{
    AsyncWriting,
    handler::CONFIG_PATH,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_read_pair_record(writer: &mut impl AsyncWriting, usbmux_packet: &UsbMuxPacket) {
    let tag = usbmux_packet.header.tag;

    let pair_record_id = match usbmux_packet
        .payload
        .as_plist()
        .and_then(|p| p.as_dictionary())
        .and_then(|d| d.get("PairRecordID"))
        .and_then(|v| v.as_string())
    {
        Some(id) => id,
        None => {
            error!(tag, "Invalid or missing PairRecordID");
            return;
        }
    };

    debug!(tag, pair_record_id, "Reading pair record");

    let path = format!("{CONFIG_PATH}/lockdown/{pair_record_id}.plist");

    trace!(tag, path, "Reading pairing file");

    let pairing_file = match tokio::fs::read(&path).await {
        Ok(data) => data,
        Err(e) => {
            error!(
                tag,
                pair_record_id,
                path,
                err = ?e,
                "Failed to read pairing file"
            );
            return;
        }
    };

    trace!(
        tag,
        pair_record_id,
        size = pairing_file.len(),
        "Pairing file loaded"
    );

    let pairing_file_xml = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
        "PairRecordData": pairing_file
    }));

    let usbmux_packet = UsbMuxPacket::encode_from(
        pairing_file_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        usbmux_packet.header.tag,
    );

    trace!(tag, "Sending pair record response");

    if let Err(e) = writer.write_all(&usbmux_packet).await {
        error!(
            tag,
            pair_record_id,
            err = ?e,
            "Failed to write response"
        );
        return;
    }

    if let Err(e) = writer.flush().await {
        error!(
            tag,
            pair_record_id,
            err = ?e,
            "Failed to flush response"
        );
        return;
    }

    trace!(tag, pair_record_id, "Pair record sent successfully");
}
