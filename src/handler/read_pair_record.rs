use std::io::ErrorKind;

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, ResultCode, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace, warn};

pub async fn handle_read_pair_record(
    writer: &mut impl AsyncWriting,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    if let Err(e) = read_pair_record(writer, pair_record_id, tag).await {
        match e {
            RusbmuxError::UnexpectedPacket(_) => {
                send_result(writer, ResultCode::BadCommand, tag).await?;
            }
            RusbmuxError::IO(ref e)
                if matches!(e.kind(), ErrorKind::PermissionDenied | ErrorKind::NotFound) =>
            {
                send_result(writer, ResultCode::BadDeviceOrNoSuchFile, tag).await?;
            }
            _ => {}
        }

        return Err(e);
    }

    Ok(())
}

pub async fn read_pair_record(
    writer: &mut impl AsyncWriting,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    trace!(tag, pair_record_id, "Reading pair record");

    if pair_record_id.contains('/')
        || pair_record_id.contains('\\')
        || pair_record_id.contains("..")
    {
        warn!(?pair_record_id, "malicious pair record id detected");
        return Err(RusbmuxError::UnexpectedPacket(
            "Given pair record id is malformed".into(),
        ));
    }

    let path = format!("{LOCKDOWN_PATH}/{pair_record_id}.plist");

    trace!(tag, path, "Reading pairing file");

    let pairing_file = tokio::fs::read(&path).await.inspect_err(|e| {
        error!(
            tag,
            pair_record_id,
            path,
            err = ?e,
            "Failed to read pairing file"
        );
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
        tag,
    );

    trace!(tag, "Sending pair record response");

    writer.write_all(&usbmux_packet).await.inspect_err(|e| {
        if !crate::utils::is_disconnect_io(e) {
            error!(
                tag,
                pair_record_id,
                err = ?e,
                "Failed to write read pair record response"
            );
        }
    })?;

    debug!(tag, pair_record_id, "Pair record sent");

    Ok(())
}
