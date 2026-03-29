use std::io::ErrorKind;

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{CONFIG_PATH, ResultCode, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_read_pair_record(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    if let Err(e) = read_pair_record(writer, usbmux_packet).await {
        match e {
            RusbmuxError::ValueNotFound("PairRecordID") => {
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
        };

        return Err(e);
    }

    Ok(())
}

pub async fn read_pair_record(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    let tag = usbmux_packet.header.tag;

    let pair_record_id = usbmux_packet
        .payload
        .as_plist()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a plist payload".to_string(),
        ))?
        .as_dictionary()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a dictionary plist payload".to_string(),
        ))?
        .get("PairRecordID")
        .ok_or(RusbmuxError::ValueNotFound("PairRecordID"))?
        .as_string()
        .ok_or(RusbmuxError::InvalidData("PairRecordID is not a string"))?;

    trace!(tag, pair_record_id, "Reading pair record");

    let path = format!("{CONFIG_PATH}/lockdown/{pair_record_id}.plist");

    trace!(tag, path, "Reading pairing file");

    let pairing_file = tokio::fs::read(&path).await.inspect_err(|e| {
        error!(
            tag,
            pair_record_id,
            path,
            err = ?e,
            "Failed to read pairing file"
        )
    })?;

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

    writer.write_all(&usbmux_packet).await.inspect_err(|e| {
        error!(
            tag,
            pair_record_id,
            err = ?e,
            "Failed to write read pair record response"
        )
    })?;

    writer.flush().await.inspect_err(|e| {
        error!(
            tag,
            pair_record_id,
            err = ?e,
            "Failed to flush read pair record response"
        )
    })?;

    debug!(tag, pair_record_id, "Pair record sent");

    Ok(())
}
