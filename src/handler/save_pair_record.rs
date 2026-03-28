use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, trace};

use crate::{
    AsyncWriting,
    handler::{CONFIG_PATH, send_result_okay},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

pub async fn handle_save_pair_record(writer: &mut impl AsyncWriting, usbmux_packet: &UsbMuxPacket) {
    let tag = usbmux_packet.header.tag;

    let pair_record_info = match usbmux_packet
        .payload
        .as_plist()
        .and_then(|p| p.as_dictionary())
    {
        Some(dict) => dict,
        None => {
            error!(tag, "Invalid plist payload");
            return;
        }
    };

    let pair_record_id = match pair_record_info
        .get("PairRecordID")
        .and_then(|v| v.as_string())
    {
        Some(id) => id,
        None => {
            error!(tag, "Missing or invalid PairRecordID");
            return;
        }
    };

    let pair_record_data = match pair_record_info
        .get("PairRecordData")
        .and_then(|v| v.as_data())
    {
        Some(data) => data,
        None => {
            error!(tag, pair_record_id, "Missing or invalid PairRecordData");
            return;
        }
    };

    debug!(
        tag,
        pair_record_id,
        data_len = pair_record_data.len(),
        "Received pair record data"
    );

    let parsed_plist = match plist::from_bytes::<plist::Value>(pair_record_data) {
        Ok(p) => p,
        Err(e) => {
            error!(
                tag,
                pair_record_id,
                err = ?e,
                "Failed to parse PairRecordData"
            );
            return;
        }
    };

    let path = format!("{CONFIG_PATH}/lockdown/{pair_record_id}.plist");

    trace!(tag, pair_record_id, path, "Writing pair record to disk");

    if let Err(e) =
        tokio::fs::write(&path, plist_macro::plist_value_to_xml_bytes(&parsed_plist)).await
    {
        error!(
            tag,
            pair_record_id,
            path,
            err = ?e,
            "Failed to write pair record file"
        );
        return;
    }

    info!(tag, pair_record_id, "Pair record saved successfully");

    // send a paired message if the `DeviceID` is provided, it's not necessary, but it's there for
    // backword compatibility
    if let Some(device_id) = pair_record_info
        .get("DeviceID")
        .and_then(|id| id.as_unsigned_integer())
    {
        debug!(tag, pair_record_id, device_id, "Sending paired message");

        let pair_response = UsbMuxPacket::encode_from(
            plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
                "MessageType": "Paired",
                "DeviceID": device_id
            })),
            UsbMuxVersion::Plist,
            UsbMuxMsgType::MessagePlist,
            tag,
        );

        if let Err(e) = writer.write_all(&pair_response).await {
            error!(
                tag,
                pair_record_id,
                err = ?e,
                "Failed to send paired response"
            );
            return;
        }

        if let Err(e) = writer.flush().await {
            error!(
                tag,
                pair_record_id,
                err = ?e,
                "Failed to flush paired response"
            );
            return;
        }

        trace!(tag, pair_record_id, "Paired response sent");
    }

    trace!(tag, "Sending result OKAY back to client");

    send_result_okay(writer, tag).await;
}
