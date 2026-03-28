use crate::{
    AsyncWriting,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
    watcher::HOTPLUG_EVENT_TX,
};
use tokio::io::AsyncWriteExt;
use tracing::{error, trace};

pub async fn handle_listeners_list(writer: &mut impl AsyncWriting, tag: u32) {
    let event_tx = match HOTPLUG_EVENT_TX.get() {
        Some(tx) => tx,
        None => {
            error!(tag, "HOTPLUG_EVENT_TX not initialized");
            return;
        }
    };

    let mut listeners_plist = vec![];
    for _ in 0..event_tx.receiver_count() {
        listeners_plist.push(plist_macro::plist!({
            "Blacklisted": false,
            "ConnType": 0,
            "ID String": "unknown",
            "ProgName": "unknown",
        }));
    }

    let listeners_plist_result = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
        "ListenerList": listeners_plist
    }));

    let usbmux_packet = UsbMuxPacket::encode_from(
        listeners_plist_result,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );

    trace!(tag, "Sending listeners list response");

    if let Err(e) = writer.write_all(&usbmux_packet).await {
        error!(
            tag,
            err = ?e,
            "Failed to write listeners list packet"
        );
        return;
    }

    if let Err(e) = writer.flush().await {
        error!(
            tag,
            err = ?e,
            "Failed to flush listeners list packet"
        );
        return;
    }

    trace!(
        tag,
        listeners = event_tx.receiver_count(),
        "Listeners list sent successfully"
    );
}
