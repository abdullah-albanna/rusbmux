use std::{io::ErrorKind, path::Path};

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxResult, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace, warn};

pub async fn handle_read_pair_record(
    writer: &mut impl AsyncWriting,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    if let Err(err) = read_pair_record(writer, pair_record_id, tag).await {
        match err {
            RusbmuxError::UnexpectedPacket(_) => {
                send_result(writer, UsbMuxResult::BadCommand, tag).await?;
            }
            RusbmuxError::IO(ref io_err)
                if matches!(
                    io_err.kind(),
                    ErrorKind::PermissionDenied | ErrorKind::NotFound
                ) =>
            {
                send_result(writer, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await?;
            }
            _ => {}
        }

        return Err(err);
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

    let path = Path::new(LOCKDOWN_PATH).join(format!("{pair_record_id}.plist"));

    trace!(tag, ?path, "Reading pairing file");

    let pairing_file = tokio::fs::read(&path).await.inspect_err(|err| {
        error!(
            tag,
            pair_record_id,
            ?path,
            %err,
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

    writer.write_all(&usbmux_packet).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(
                tag,
                pair_record_id,
                %err,
                "Failed to write read pair record response"
            );
        }
    })?;

    debug!(tag, pair_record_id, "Pair record sent");

    Ok(())
}
