use std::io::ErrorKind;

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, ResultCode, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

pub async fn handle_save_pair_record(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    match save_pair_record(writer, usbmux_packet).await {
        Ok(()) => {
            send_result(writer, ResultCode::OK, usbmux_packet.header.tag).await?;
        }

        Err(e) => {
            match e {
                RusbmuxError::ValueNotFound("PairRecordID" | "PairRecordData")
                | RusbmuxError::InvalidData(_) => {
                    send_result(writer, ResultCode::InvalidInput, usbmux_packet.header.tag).await?;
                }

                RusbmuxError::IO(ref e)
                    if matches!(e.kind(), ErrorKind::PermissionDenied | ErrorKind::NotFound) =>
                {
                    send_result(
                        writer,
                        ResultCode::BadDeviceOrNoSuchFile,
                        usbmux_packet.header.tag,
                    )
                    .await?;
                }

                _ => {}
            }

            return Err(e);
        }
    }

    Ok(())
}

pub async fn save_pair_record(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    let tag = usbmux_packet.header.tag;

    let pair_record_info = usbmux_packet
        .payload
        .as_plist()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a plist payload".to_string(),
        ))?
        .as_dictionary()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a dictionary plist payload".to_string(),
        ))?;

    let pair_record_id = pair_record_info
        .get("PairRecordID")
        .ok_or(RusbmuxError::ValueNotFound("PairRecordID"))?
        .as_string()
        .ok_or(RusbmuxError::InvalidData("PairRecordID is not a string"))?;

    let pair_record_data = pair_record_info
        .get("PairRecordData")
        .ok_or(RusbmuxError::ValueNotFound("PairRecordData"))?
        .as_data()
        .ok_or(RusbmuxError::InvalidData("PairRecordData is not a data"))?;

    trace!(
        tag,
        pair_record_id,
        data_len = pair_record_data.len(),
        "Received pair record data"
    );

    let parsed_plist = plist::from_bytes::<plist::Value>(pair_record_data).inspect_err(|e| {
        error!(
            tag,
            pair_record_id,
            err = ?e,
            "Failed to parse PairRecordData"
        );
    })?;

    let path = format!("{LOCKDOWN_PATH}/{pair_record_id}.plist");

    trace!(tag, pair_record_id, path, "Writing pair record to disk");

    tokio::fs::write(&path, plist_macro::plist_value_to_xml_bytes(&parsed_plist))
        .await
        .inspect_err(|e| {
            error!(
                tag,
                pair_record_id,
                path,
                err = ?e,
                "Failed to write pair record file"
            )
        })?;

    debug!(tag, pair_record_id, "Pair record saved");

    // send a paired message if the `DeviceID` is provided, it's not necessary, but it's there for
    // backword compatibility
    if let Some(device_id) = pair_record_info
        .get("DeviceID")
        .and_then(plist::Value::as_unsigned_integer)
    {
        trace!(tag, pair_record_id, device_id, "Sending paired message");

        let pair_response = UsbMuxPacket::encode_from(
            plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
                "MessageType": "Paired",
                "DeviceID": device_id
            })),
            UsbMuxVersion::Plist,
            UsbMuxMsgType::MessagePlist,
            tag,
        );

        writer.write_all(&pair_response).await.inspect_err(|e| {
            error!(
                tag,
                pair_record_id,
                err = ?e,
                "Failed to send paired response"
            );
        })?;

        writer.flush().await.inspect_err(|e| {
            error!(
                tag,
                pair_record_id,
                err = ?e,
                "Failed to flush paired response"
            );
        })?;

        trace!(tag, pair_record_id, "Paired response sent");
    } else {
        trace!(tag, "DeviceID is not provided, skipping the paired message");
    }

    Ok(())
}
