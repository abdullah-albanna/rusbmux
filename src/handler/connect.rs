use crate::{
    AsyncReading, AsyncWriting, ReadWrite,
    handler::send_result_okay,
    parser::{device_mux::DeviceMuxPacket, usbmux::UsbMuxPacket},
    watcher::CONNECTED_DEVICES,
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    let mut read_buf = BytesMut::with_capacity(40_000);
    loop {
        tokio::select! {
            packet = conn.recv() => {
                client_send(&mut client, packet).await;
            }

            client_packet = client_read(&mut client, &mut read_buf) => {
                if client_packet.is_empty() {
                    conn.close().await;
                    break;
                }

                conn.send_bytes(client_packet).await;
            }
        };
    }
}

pub async fn client_read(client: &mut impl AsyncReading, buf: &mut BytesMut) -> Bytes {
    buf.clear();

    // SAFETY: read() will only write initialized bytes up to n
    let spare_len = buf.spare_capacity_mut().len();
    // fill with uninit is fine, read() tells us how many bytes are valid
    unsafe { buf.set_len(spare_len) };

    let n = client.read(buf).await.unwrap();
    buf.truncate(n);

    buf.clone().freeze()
}

pub async fn client_send(client: &mut impl AsyncWriting, packet: DeviceMuxPacket) {
    client
        .write_all(packet.payload.as_raw().unwrap())
        .await
        .unwrap();
    client.flush().await.unwrap();
}
