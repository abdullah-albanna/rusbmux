use std::io::ErrorKind;

use tracing::{debug, error};

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{CONFIG_PATH, ResultCode, send_result},
    parser::usbmux::UsbMuxPacket,
};

pub async fn handle_delete_pair_record(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    match delete_pair_record(usbmux_packet).await {
        Ok(()) => send_result(writer, ResultCode::OK, usbmux_packet.header.tag).await?,
        Err(e) => {
            match e {
                RusbmuxError::ValueNotFound("PairRecordID") | RusbmuxError::InvalidData(_) => {
                    send_result(writer, ResultCode::InvalidInput, usbmux_packet.header.tag).await?
                }

                RusbmuxError::IO(ref e) if e.kind() == ErrorKind::NotFound => {
                    send_result(
                        writer,
                        ResultCode::BadDeviceOrNoSuchFile,
                        usbmux_packet.header.tag,
                    )
                    .await?
                }
                _ => {}
            };
            return Err(e);
        }
    }

    Ok(())
}

pub async fn delete_pair_record(usbmux_packet: &UsbMuxPacket) -> Result<(), RusbmuxError> {
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

    debug!(
        tag = usbmux_packet.header.tag,
        pair_record_id, "Deleting pair record"
    );

    let path = format!("{CONFIG_PATH}/lockdown/{pair_record_id}.plist");

    tokio::fs::remove_file(&path).await.inspect_err(|e| error!(tag = usbmux_packet.header.tag, pair_record_id, path, err = ?e, "Failed to delete pair record"))?;

    debug!(
        tag = usbmux_packet.header.tag,
        pair_record_id, path, "Pair record deleted"
    );

    Ok(())
}
