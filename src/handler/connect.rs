use crate::{
    ReadWrite,
    device::CONNECTED_DEVICES,
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

    let mut connected_devices = CONNECTED_DEVICES.write().await;

    let dev = connected_devices
        .iter_mut()
        .find(|dev| dev.inner.id == device_id as u32)
        .unwrap();

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

    start_connect_loop(dev, client).await;
}

pub async fn start_connect_loop(_dev: &mut crate::device::Device, client: &mut impl ReadWrite) {
    loop {
        let mut len_buff = [0u8; 4];
        client.read_exact(&mut len_buff).await.unwrap();

        let payload_len = u32::from_be_bytes(len_buff) as usize;

        let mut payload = vec![0u8; payload_len];

        client.read_exact(&mut payload).await.unwrap();

        dbg!(plist::from_bytes::<plist::Value>(&payload).unwrap());
    }
}
