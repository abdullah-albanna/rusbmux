use crate::{
    AsyncWriting,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;

pub async fn handle_read_pair_record(writer: &mut impl AsyncWriting, usbmux_packet: &UsbMuxPacket) {
    let payload_plist = usbmux_packet
        .payload
        .clone()
        .as_plist()
        .expect("`ReadPairRecord` payload was not a plist");

    let pair_record_id = payload_plist
        .as_dictionary()
        .expect("`ReadPairRecord` payload plist was not a dictionay")
        .get("PairRecordID")
        .expect("`PairRecordID` was not in the `ReadPairRecord` plist payload")
        .as_string()
        .expect("`PairRecordID` was not a string");

    let (pair_record_id_first, pair_record_id_rest) = pair_record_id.split_at(8);

    let pairing_file = tokio::fs::read(format!(
        "/var/lib/lockdown/{pair_record_id_first}-{pair_record_id_rest}.plist"
    ))
    .await
    .expect("pairing file does not exists");

    let pairing_file_xml = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
        "PairRecordData": pairing_file
    }));

    let usbmux_packet = UsbMuxPacket::encode_from(
        pairing_file_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::DevicePaired,
        usbmux_packet.header.tag,
    );

    writer.write_all(&usbmux_packet).await.unwrap();
    writer.flush().await.unwrap();
}
