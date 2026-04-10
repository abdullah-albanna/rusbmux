use std::sync::Arc;

use crate::{
    AsyncReading, AsyncWriting, ReadWrite,
    device::conn::DeviceMuxConn,
    error::RusbmuxError,
    handler::send_result,
    parser::{device_mux::DeviceMuxPacket, usbmux::UsbMuxPacket},
    usb::MAX_PACKET_PAYLOAD_SIZE,
    watcher::CONNECTED_DEVICES,
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, trace};

use super::ResultCode;

const CLIENT_BUFF_SIZE: usize = 128 * 1024;

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

    let mut read_buf = BytesMut::with_capacity(CLIENT_BUFF_SIZE);
    let (r, mut w) = tokio::io::split(client);
    let mut r = BufReader::new(r);
    loop {
        tokio::select! {
            biased;

            _ = conn.wait_shutdown() => {
                debug!(device_id, port_number, "Shutting down connection");
                return Err(RusbmuxError::DeviceNotFound(device_id));
            }

            packet = conn.recv() => {
                let packet = packet?;
                trace!(device_id, port_number, "Received packet from device");

                client_send(&mut w, packet).await?;
            }

            client_packet = client_read(&mut r, &mut read_buf, conn.get_sendable_bytes()),
                            if conn.get_sendable_bytes() > 0
            => {
                let client_packet = client_packet?;

                if client_packet.is_empty() {
                    info!(device_id, port_number, "Client disconnected");
                    conn.close().await?;
                    return Ok(());
                }

                debug!(device_id, port_number, "Processing client packet");

                conn.send_bytes(client_packet).await?;
            }
        };
    }
}

pub async fn client_read(
    client: &mut impl AsyncReading,
    buf: &mut BytesMut,
    sendable_bytes: usize,
) -> Result<Bytes, RusbmuxError> {
    loop {
        if !buf.is_empty() {
            return Ok(buf
                .split_to(sendable_bytes.min(buf.len()).min(MAX_PACKET_PAYLOAD_SIZE))
                .freeze());
        }

        buf.reserve(CLIENT_BUFF_SIZE);
        client
            .read_buf(buf)
            .await
            .inspect_err(|e| error!(err = ?e, "Failed to read from client"))?;
    }
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
