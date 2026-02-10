use crate::{
    AsyncWriting,
    handler::device_watcher::HOTPLUG_EVENT_TX,
    parser::usbmux::{UsbMuxHeader, UsbMuxMsgType, UsbMuxPacket, UsbMuxPayload, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;

pub async fn handle_listeners_list(writer: &mut impl AsyncWriting, tag: u32) {
    let event_tx = HOTPLUG_EVENT_TX.get().unwrap();
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

    let usbmux_packet = UsbMuxPacket {
        header: UsbMuxHeader {
            len: (listeners_plist_result.len() + UsbMuxHeader::SIZE) as _,
            version: UsbMuxVersion::Plist,
            msg_type: UsbMuxMsgType::MessagePlist,
            tag,
        },
        payload: UsbMuxPayload::Raw(listeners_plist_result),
    };

    writer.write_all(&usbmux_packet.encode()).await.unwrap();
    writer.flush().await.unwrap();
}
