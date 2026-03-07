use crate::{
    AsyncWriting,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;

pub async fn handle_read_buid(writer: &mut impl AsyncWriting, usbmux_packet: &UsbMuxPacket) {
    let system_config =
        plist::from_file::<_, plist::Value>("/var/lib/lockdown/SystemConfiguration.plist").unwrap();

    let buid = system_config
        .as_dictionary()
        .expect("`SystemConfiguration.plist` file is not a dictionary")
        .get("SystemBUID")
        .expect("`SystemBUID` is not found in the `SystemConfiguration.plist` file")
        .as_string()
        .expect("`SystemBUID` is not a string");

    let response_plist = plist_macro::plist!({
        "BUID": buid
    });

    let usbmux_packet = UsbMuxPacket::encode_from(
        plist_macro::plist_value_to_xml_bytes(&response_plist),
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        usbmux_packet.header.tag,
    );

    writer.write_all(&usbmux_packet).await.unwrap();
    writer.flush().await.unwrap();
}
