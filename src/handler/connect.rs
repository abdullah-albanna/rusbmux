use crate::{
    AsyncReading, AsyncWriting, ReadWrite,
    handler::send_result_okay,
    parser::{device_mux::DeviceMuxPacket, usbmux::UsbMuxPacket},
    watcher::CONNECTED_DEVICES,
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, trace};

const CHUNK_SIZE: usize = 32 * 1024;

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

    info!(
        device_id,
        port_number,
        tag = usbmux_packet.header.tag,
        "Client connecting"
    );

    let connected_devices = CONNECTED_DEVICES.read().await;

    let device = match connected_devices.iter().find(|dev| dev.id == device_id) {
        Some(dev) => dev,
        None => {
            // TODO: send back result
            error!(device_id, port_number, "Device not found");
            return;
        }
    };

    let conn = device.connect(port_number).await;

    drop(connected_devices);

    send_result_okay(&mut client, usbmux_packet.header.tag).await;

    let mut read_buf = BytesMut::with_capacity(128 * 1024);
    let (r, mut w) = tokio::io::split(client);
    let mut r = BufReader::new(r);
    loop {
        tokio::select! {
            packet = conn.recv() => {
                trace!( device_id, port_number, "Received packet from device");

                if client_send(&mut w, packet).await.is_none() {
                    break;
                }
            }

            client_packet = client_read(&mut r, &mut read_buf) => {
                let Some(client_packet) = client_packet else {
                    break;
                };

                if client_packet.is_empty() {
                    info!( device_id, port_number, "Client disconnected");
                    conn.close().await;
                    break;
                }

                let mut len = client_packet.len();
                let mut last_i = 0;

                debug!( device_id, port_number, len, "Processing client packet");
                while len > CHUNK_SIZE {
                    trace!( device_id, port_number, chunk_size = CHUNK_SIZE, "Sending packet chunk");
                    conn.send_bytes(client_packet.slice(last_i..(last_i + CHUNK_SIZE))).await;
                    len -= CHUNK_SIZE;
                    last_i += CHUNK_SIZE;
                };


                if len > 0 {
                    trace!( device_id, port_number, chunk_size = len, "Sending remaining packet chunk");
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

        Err(e) => {
            error!( err = ?e, "Failed to read from client");
            None
        }
    }
}

pub async fn client_send(client: &mut impl AsyncWriting, packet: DeviceMuxPacket) -> Option<()> {
    let payload = packet.payload.encode();

    trace!(len = payload.len(), "Sending packet to client");

    if let Err(e) = client.write_all(&payload).await {
        error!( err = ?e, "Failed to write packet to client");
        return None;
    }

    if let Err(e) = client.flush().await {
        error!( err = ?e, "Failed to flush client");
        return None;
    }

    Some(())
}
