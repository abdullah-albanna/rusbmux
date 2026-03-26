use crate::{
    AsyncReading, AsyncWriting, ReadWrite,
    handler::send_result_okay,
    parser::{device_mux::DeviceMuxPacket, usbmux::UsbMuxPacket},
    watcher::CONNECTED_DEVICES,
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

pub async fn handle_connect(mut client: Box<dyn ReadWrite>, usbmux_packet: UsbMuxPacket) {
    let client_payload = usbmux_packet.payload.as_plist().unwrap();
    let client_payload_dict = client_payload.as_dictionary().unwrap();

    let port_number = client_payload_dict
        .get("PortNumber")
        .map(|v| {
            if let Some(ui) = v.as_unsigned_integer() {
                ui as u16
            } else if let Some(si) = v.as_signed_integer() {
                si as u16
            } else {
                panic!("PortNumber is neither a signed number nor an unsigned number");
            }
        })
        .unwrap()
        .to_be();

    let device_id = client_payload_dict
        .get("DeviceID")
        .unwrap()
        .as_unsigned_integer()
        .unwrap();

    println!("port number: {port_number}, device id: {device_id}");

    let connected_devices = CONNECTED_DEVICES.read().await;

    let device = connected_devices
        .iter()
        .find(|dev| dev.id == device_id)
        .unwrap();

    let conn = device.connect(port_number).await;

    drop(connected_devices);

    send_result_okay(&mut client, usbmux_packet.header.tag).await;

    let mut read_buf = BytesMut::with_capacity(128 * 1024);
    let (r, mut w) = tokio::io::split(client);
    let mut r = BufReader::new(r);
    loop {
        tokio::select! {
            packet = conn.recv() => {
                client_send(&mut w, packet).await;
            }

            Some(client_packet) = client_read(&mut r, &mut read_buf) => {
                if client_packet.is_empty() {
                    conn.close().await;
                    break;
                }

                let mut len = client_packet.len();
                let mut last_i = 0;
                while len > 32 * 1024 {
                    conn.send_bytes(client_packet.slice(last_i..(last_i + (32 * 1024)))).await;
                    len -= 32 * 1024;
                    last_i += 32 * 1024;
                };


                if len > 0 {
                    conn.send_bytes(client_packet.slice(last_i..)).await;
                }
            }
        };
    }
}

pub async fn client_read(client: &mut impl AsyncReading, buf: &mut BytesMut) -> Option<Bytes> {
    buf.reserve(128 * 1024);
    match client.read_buf(buf).await {
        Ok(_) => Some(buf.split().freeze()),

        Err(_) => None,
    }
}

pub async fn client_send(client: &mut impl AsyncWriting, packet: DeviceMuxPacket) {
    client
        .write_all(packet.payload.as_bytes().unwrap())
        .await
        .unwrap();
    client.flush().await.unwrap();
}
