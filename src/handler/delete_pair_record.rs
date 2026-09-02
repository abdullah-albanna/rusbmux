use std::{io::ErrorKind, path::Path};

use tracing::{debug, error, warn};

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, send_result},
    parser::usbmux::UsbMuxResult,
};

pub async fn handle_delete_pair_record(
    writer: &mut impl AsyncWriting,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    match delete_pair_record(pair_record_id, tag).await {
        Ok(()) => send_result(writer, UsbMuxResult::OK, tag).await?,
        Err(err) => {
            match err {
                RusbmuxError::UnexpectedPacket(_) => {
                    send_result(writer, UsbMuxResult::BadCommand, tag).await?;
                }

                RusbmuxError::IO(ref io_err) if io_err.kind() == ErrorKind::NotFound => {
                    send_result(writer, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await?;
                }
                _ => {}
            }
            return Err(err);
        }
    }

    Ok(())
}

pub async fn delete_pair_record(pair_record_id: String, tag: u32) -> Result<(), RusbmuxError> {
    debug!(tag, pair_record_id, "Deleting pair record");

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

    tokio::fs::remove_file(&path).await.inspect_err(
        |err| error!(tag, pair_record_id, ?path, %err, "Failed to delete pair record"),
    )?;

    debug!(tag, pair_record_id, ?path, "Pair record deleted");

    Ok(())
}
