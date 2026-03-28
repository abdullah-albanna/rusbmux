use tracing::{debug, error, info, trace};

use crate::{
    AsyncWriting,
    handler::{CONFIG_PATH, send_result_okay},
    parser::usbmux::UsbMuxPacket,
};

pub async fn handle_delete_pair_record(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) {
    let pair_record_id = match usbmux_packet
        .payload
        .as_plist()
        .and_then(|plist| plist.as_dictionary())
        .and_then(|dict| dict.get("PairRecordID"))
        .and_then(|v| v.as_string())
    {
        Some(id) => id,
        None => {
            error!(
                tag = usbmux_packet.header.tag,
                "PairRecordID missing or invalid"
            );
            return;
        }
    };

    debug!(
        tag = usbmux_packet.header.tag,
        pair_record_id, "Deleting pair record"
    );

    let path = format!("{CONFIG_PATH}/lockdown/{pair_record_id}.plist");

    match tokio::fs::remove_file(&path).await {
        Ok(_) => {
            info!(
                tag = usbmux_packet.header.tag,
                pair_record_id, path, "Pair record deleted successfully"
            );
        }
        Err(e) => {
            // TODO: send result
            error!( tag = usbmux_packet.header.tag, pair_record_id, path, err = ?e, "Failed to delete pair record");
        }
    };

    trace!(
        tag = usbmux_packet.header.tag,
        pair_record_id, "Sending result OKAY back to client"
    );
    send_result_okay(writer, usbmux_packet.header.tag).await;
}
