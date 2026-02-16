use std::io::ErrorKind;

use crate::{
    ReadWrite,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn handle_connect(client: &mut impl ReadWrite, usbmux_packet: UsbMuxPacket) {
    let client_payload = usbmux_packet.payload.as_plist().unwrap();
    let client_payload_dict = client_payload.as_dictionary().unwrap();

    let port_number = client_payload_dict
        .get("PortNumber")
        .unwrap()
        .as_unsigned_integer()
        .unwrap();

    let device_id = client_payload_dict
        .get("DeviceID")
        .unwrap()
        .as_unsigned_integer()
        .unwrap();

    println!("port number: {port_number}, device id: {device_id}");
    let connect_plist_response = plist_macro::plist!({
        "Number": 0
    });

    let connect_response_packet = UsbMuxPacket::encode_from(
        plist_macro::plist_value_to_xml_bytes(&connect_plist_response),
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        usbmux_packet.header.tag,
    );

    client.write_all(&connect_response_packet).await.unwrap();
    client.flush().await.unwrap();

    start_connect_loop(client, port_number, device_id).await;
}

pub async fn start_connect_loop(client: &mut impl ReadWrite, port_number: u64, device_id: u64) {
    loop {
        let mut len_buff = [0u8; 4];
        client.read_exact(&mut len_buff).await.unwrap();

        let payload_len = u32::from_be_bytes(len_buff) as usize;

        let mut payload = vec![0u8; payload_len];

        client.read_exact(&mut payload).await.unwrap();

        let payload_plist = plist::from_bytes::<plist::Value>(&payload).unwrap();

        println!("{payload_plist:#?}");
    }
}
