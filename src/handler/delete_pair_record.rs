use crate::{AsyncWriting, handler::send_result_okay, parser::usbmux::UsbMuxPacket};

pub async fn handle_delete_pair_record(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) {
    let pair_record_id = usbmux_packet
        .payload
        .as_plist()
        .expect("`DeletePairRecord` payload was not a plist")
        .as_dictionary()
        .expect("`DeletePairRecord` payload plist was not a dictionay")
        .get("PairRecordID")
        .expect("`PairRecordID` was not in the `DeletePairRecord` plist payload")
        .as_string()
        .expect("`PairRecordID` was not a string");

    tokio::fs::remove_file(format!("/var/lib/lockdown/{pair_record_id}.plist"))
        .await
        .unwrap();

    send_result_okay(writer, usbmux_packet.header.tag).await;
}
