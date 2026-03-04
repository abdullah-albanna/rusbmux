use crate::{
    AsyncReading, AsyncWriting, ReadWrite,
    device::CONNECTED_DEVICES,
    handler::send_result_okay,
    parser::{device_mux::DeviceMuxPacket, usbmux::UsbMuxPacket},
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn handle_connect(mut client: Box<dyn ReadWrite>, usbmux_packet: UsbMuxPacket) {
    let client_payload = usbmux_packet.payload.as_plist().unwrap();
    let client_payload_dict = client_payload.as_dictionary().unwrap();

    let port_number = (client_payload_dict
        .get("PortNumber")
        .unwrap()
        .as_unsigned_integer()
        .unwrap() as u16)
        .to_be();

    let device_id = client_payload_dict
        .get("DeviceID")
        .unwrap()
        .as_unsigned_integer()
        .unwrap();

    println!("port number: {port_number}, device id: {device_id}");

    // TODO: only take &mut when needed instead
    let mut connected_devices = CONNECTED_DEVICES.write().await;

    let dev = connected_devices
        .iter_mut()
        .find(|dev| dev.inner.id == device_id)
        .unwrap();

    let mut conn = dev.connect(port_number).await;

    send_result_okay(&mut client, usbmux_packet.header.tag).await;

    loop {
        tokio::select! {
            packet = conn.recv() => {
                dbg!(&packet);
                client_send(&mut client, packet).await;
            }

            client_packet = client_read(&mut client) => {
                dbg!(&client_packet);
                if client_packet.is_empty() {
                    conn.close().await;
                    break;
                }

                conn.send_bytes(client_packet).await;
            }
        };
    }
}

pub async fn client_read(client: &mut impl AsyncReading) -> Bytes {
    let mut payload = BytesMut::with_capacity(49116);

    payload.resize(49116, 0);

    let n = client.read(&mut payload).await.unwrap();

    payload.resize(n, 0);
    payload.freeze()
}

pub async fn client_send(client: &mut impl AsyncWriting, packet: DeviceMuxPacket) {
    client
        .write_all(packet.payload.as_raw().unwrap())
        .await
        .unwrap();
    client.flush().await.unwrap();
}
