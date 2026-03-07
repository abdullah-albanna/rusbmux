use tokio::io::AsyncWriteExt;

use crate::{
    AsyncWriting,
    handler::{CONFIG_PATH, send_result_okay},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

pub async fn handle_save_pair_record(writer: &mut impl AsyncWriting, usbmux_packet: &UsbMuxPacket) {
    let pair_record_info = usbmux_packet
        .payload
        .as_plist()
        .expect("`SavePairRecord` payload was not a plist")
        .as_dictionary()
        .expect("`SavePairRecord` payload plist was not a dictionay");

    let pair_record_id = pair_record_info
        .get("PairRecordID")
        .expect("`PairRecordID` was not in the `SavePairRecord` plist payload")
        .as_string()
        .expect("`PairRecordID` was not a string");

    let pair_record_data = pair_record_info
        .get("PairRecordData")
        .expect("`PairRecordData` was not in the `SavePairRecord` plist payload")
        .as_data()
        .expect("`PairRecordData` was not data");

    tokio::fs::write(
        format!("{CONFIG_PATH}/lockdown/{pair_record_id}.plist"),
        plist_macro::plist_value_to_xml_bytes(
            &plist::from_bytes::<plist::Value>(pair_record_data).unwrap(),
        ),
    )
    .await
    .expect("couldn't write to file");

    // send a paired message if the `DeviceID` is provided, it's not necessary, but it's there for
    // backword compatibility
    if let Some(device_id) = pair_record_info.get("DeviceID").map(|id| {
        id.as_unsigned_integer()
            .expect("`DeviceID` was not a number")
    }) {
        let pair_response = UsbMuxPacket::encode_from(
            plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
                "MessageType": "Paired",
                "DeviceID": device_id
            })),
            UsbMuxVersion::Plist,
            UsbMuxMsgType::MessagePlist,
            usbmux_packet.header.tag,
        );

        writer.write_all(&pair_response).await.unwrap();
        writer.flush().await.unwrap();
    }

    send_result_okay(writer, usbmux_packet.header.tag).await;
}
