use std::io::ErrorKind;

use tracing::{debug, error, warn};

use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, ResultCode, send_result},
};

pub async fn handle_delete_pair_record(
    writer: &mut impl AsyncWriting,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    match delete_pair_record(pair_record_id, tag).await {
        Ok(()) => send_result(writer, ResultCode::OK, tag).await?,
        Err(e) => {
            match e {
                RusbmuxError::UnexpectedPacket(_) => {
                    send_result(writer, ResultCode::BadCommand, tag).await?;
                }

                RusbmuxError::IO(ref e) if e.kind() == ErrorKind::NotFound => {
                    send_result(writer, ResultCode::BadDeviceOrNoSuchFile, tag).await?;
                }
                _ => {}
            }
            return Err(e);
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

    let path = format!("{LOCKDOWN_PATH}/{pair_record_id}.plist");

    tokio::fs::remove_file(&path).await.inspect_err(
        |e| error!(tag, pair_record_id, path, err = ?e, "Failed to delete pair record"),
    )?;

    debug!(tag, pair_record_id, path, "Pair record deleted");

    Ok(())
}
