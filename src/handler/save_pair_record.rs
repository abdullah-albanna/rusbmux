use std::io::ErrorKind;

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace, warn};

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, ResultCode, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

pub async fn handle_save_pair_record(
    writer: &mut impl AsyncWriting,
    pair_record_id: String,
    pair_record_data: plist::Data,
    device_id: Option<u64>,
    tag: u32,
) -> Result<(), RusbmuxError> {
    match save_pair_record(writer, pair_record_id, pair_record_data, device_id, tag).await {
        Ok(()) => {
            send_result(writer, ResultCode::OK, tag).await?;
        }

        Err(e) => {
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
    }

    Ok(())
}

pub async fn save_pair_record(
    writer: &mut impl AsyncWriting,
    pair_record_id: String,
    pair_record_data: plist::Data,
    device_id: Option<u64>,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let pair_record_data: Vec<u8> = pair_record_data.into();
    trace!(
        tag,
        pair_record_id,
        data_len = pair_record_data.len(),
        "Received pair record data"
    );

    let pair_record_data = plist::Value::Data(pair_record_data);

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

    trace!(tag, pair_record_id, path, "Writing pair record to disk");

    // TODO: permissions
    tokio::fs::write(
        &path,
        plist_macro::plist_value_to_xml_bytes(&pair_record_data),
    )
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
    if let Some(device_id) = device_id {
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
            if !crate::utils::is_disconnect_io(e) {
                error!(
                    tag,
                    pair_record_id,
                    err = ?e,
                    "Failed to send paired response"
                );
            }
        })?;

        trace!(tag, pair_record_id, "Paired response sent");
    } else {
        trace!(tag, "DeviceID is not provided, skipping the paired message");
    }

    Ok(())
}
