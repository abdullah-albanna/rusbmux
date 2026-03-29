use std::sync::Arc;

use crate::{
    AsyncReading, AsyncWriting, ReadWrite,
    device::conn::DeviceMuxConn,
    error::RusbmuxError,
    handler::send_result,
    parser::{device_mux::DeviceMuxPacket, usbmux::UsbMuxPacket},
    watcher::CONNECTED_DEVICES,
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, trace};

use super::ResultCode;

const CHUNK_SIZE: usize = 32 * 1024;

pub async fn handle_connect(
    client: &mut impl ReadWrite,
    usbmux_packet: UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    let conn = match connect(&usbmux_packet).await {
        Ok(c) => c,
        Err(e) => {
            match e {
                RusbmuxError::ValueNotFound("DeviceID")
                | RusbmuxError::DeviceNotFound(_)
                | RusbmuxError::RanOutofSourcePort => {
                    send_result(
                        client,
                        ResultCode::BadDeviceOrNoSuchFile,
                        usbmux_packet.header.tag,
                    )
                    .await?
                }
                RusbmuxError::ValueNotFound("PortNumber") => {
                    send_result(client, ResultCode::BadCommand, usbmux_packet.header.tag).await?
                }

                _ => {
                    send_result(
                        client,
                        ResultCode::ConnectionRefused,
                        usbmux_packet.header.tag,
                    )
                    .await?
                }
            }
            return Err(e);
        }
    };

    let device_id = conn.device.id;
    let port_number = conn.destination_port;

    send_result(client, ResultCode::OK, usbmux_packet.header.tag).await?;

    let mut read_buf = BytesMut::with_capacity(128 * 1024);
    let (r, mut w) = tokio::io::split(client);
    let mut r = BufReader::new(r);
    loop {
        tokio::select! {
            _ = conn.wait_shutdown() => {
                debug!(device_id, port_number, "Shutting down connection");
                break Err(RusbmuxError::DeviceNotFound(device_id));
            }

            packet = conn.recv() => {
                let packet = packet?;
                trace!(device_id, port_number, "Received packet from device");

                client_send(&mut w, packet).await?;
            }

            client_packet = client_read(&mut r, &mut read_buf) => {
                let client_packet = client_packet?;

                if client_packet.is_empty() {
                    info!(device_id, port_number, "Client disconnected");
                    conn.close().await?;
                    return Ok(());
                }

                let mut len = client_packet.len();
                let mut last_i = 0;

                debug!(device_id, port_number, len, "Processing client packet");
                while len > CHUNK_SIZE {
                    trace!( device_id, port_number, chunk_size = CHUNK_SIZE, "Sending packet chunk");
                    conn.send_bytes(client_packet.slice(last_i..(last_i + CHUNK_SIZE))).await?;
                    len -= CHUNK_SIZE;
                    last_i += CHUNK_SIZE;
                };


                if len > 0 {
                    trace!( device_id, port_number, chunk_size = len, "Sending remaining packet chunk");
                    conn.send_bytes(client_packet.slice(last_i..)).await?;
                }
            }
        };
    }
}

pub async fn connect(usbmux_packet: &UsbMuxPacket) -> Result<Arc<DeviceMuxConn>, RusbmuxError> {
    let client_payload = usbmux_packet
        .payload
        .as_plist()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a plist payload".to_string(),
        ))?;

    let client_payload_dict =
        client_payload
            .as_dictionary()
            .ok_or(RusbmuxError::UnexpectedPacket(
                "Expected a packet with a dictionary plist payload".to_string(),
            ))?;

    let device_id = client_payload_dict
        .get("DeviceID")
        .ok_or(RusbmuxError::ValueNotFound("DeviceID"))?
        .as_unsigned_integer()
        .ok_or(RusbmuxError::InvalidData(
            "DeviceID is not an unsigned integer",
        ))?;

    let port_number = client_payload_dict
        .get("PortNumber")
        .ok_or(RusbmuxError::ValueNotFound("PortNumber"))
        .map(|v| {
            if let Some(ui) = v.as_unsigned_integer() {
                Ok(ui as u16)
            } else if let Some(si) = v.as_signed_integer() {
                Ok(si as u16)
            } else {
                Err(RusbmuxError::InvalidData(
                    "PortNumber is neither a signed number nor an unsigned number",
                ))
            }
        })??
        .to_be();

    info!(
        device_id,
        port_number,
        tag = usbmux_packet.header.tag,
        "Client connecting"
    );

    let connected_devices = CONNECTED_DEVICES.read().await;

    let device = connected_devices
        .iter()
        .find(|dev| dev.id == device_id)
        .ok_or(RusbmuxError::DeviceNotFound(device_id))?;

    let conn = device.connect(port_number).await?;

    drop(connected_devices);

    Ok(conn)
}

pub async fn client_read(
    client: &mut impl AsyncReading,
    buf: &mut BytesMut,
) -> Result<Bytes, RusbmuxError> {
    buf.reserve(128 * 1024);
    Ok(client
        .read_buf(buf)
        .await
        .inspect_err(|e| error!(err = ?e, "Failed to read from client"))
        .map(|_n| buf.split().freeze())?)
}

pub async fn client_send(
    client: &mut impl AsyncWriting,
    packet: DeviceMuxPacket,
) -> Result<(), RusbmuxError> {
    let payload = packet.payload.encode();

    trace!(len = payload.len(), "Sending packet to client");

    client
        .write_all(&payload)
        .await
        .inspect_err(|e| error!(err = ?e, "Failed to write packet to client"))?;

    client
        .flush()
        .await
        .inspect_err(|e| error!(err = ?e, "Failed to flush client"))?;

    Ok(())
}
