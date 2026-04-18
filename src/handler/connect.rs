use crate::{
    AsyncReading, AsyncWriting, ReadWrite, conn::DeviceConn, device::ConectionType,
    error::RusbmuxError, handler::send_result, parser::usbmux::UsbMuxPacket,
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
                    .await?;
                }
                RusbmuxError::ValueNotFound("PortNumber") => {
                    send_result(client, ResultCode::BadCommand, usbmux_packet.header.tag).await?;
                }

                _ => {
                    send_result(
                        client,
                        ResultCode::ConnectionRefused,
                        usbmux_packet.header.tag,
                    )
                    .await?;
                }
            }
            return Err(e);
        }
    };

    send_result(client, ResultCode::OK, usbmux_packet.header.tag).await?;

    match conn.connection_type() {
        ConectionType::Usb => handle_usb_device_connect(client, conn).await?,
        ConectionType::Network => handle_network_device_connect(client, conn).await?,
    }

    Ok(())
}

#[allow(clippy::significant_drop_tightening)]
pub async fn handle_network_device_connect(
    client: &mut impl ReadWrite,
    mut conn: DeviceConn,
) -> Result<(), RusbmuxError> {
    let device_id = conn.device_id();
    let port_number = conn.dst_port();

    let net_conn = conn.as_network().unwrap();

    let mut conn_stream = net_conn.stream.lock().await;

    tokio::select! {
        res = tokio::io::copy_bidirectional_with_sizes(
            &mut *conn_stream,
            client,
            CLIENT_BUFF_SIZE,
            CLIENT_BUFF_SIZE
        ) => {
            res?;
            Ok(())
        }

        _ = net_conn.wait_shutdown() => {
            debug!(device_id, port_number, "Shutting down connection");
            Err(RusbmuxError::DeviceNotFound(device_id))
        }
    }
}

pub async fn handle_usb_device_connect(
    client: &mut impl ReadWrite,
    conn: DeviceConn,
) -> Result<(), RusbmuxError> {
    let device_id = conn.device_id();
    let port_number = conn.dst_port();

    let mut read_buf = BytesMut::with_capacity(CLIENT_BUFF_SIZE);
    let (client_reader, mut client_writer) = tokio::io::split(client);
    let mut client_reader = BufReader::new(client_reader);

    // PERF: this could be faster, I think
    // 1. run both `device->client` and `client->device` with tokio::spawn or something for parallelism
    // 2. reduce spinloops and async context switching
    loop {
        tokio::select! {
            // PERF: biased makes it slower
            biased;

            _ = conn.wait_shutdown() => {
                debug!(device_id, port_number, "Shutting down connection");
                return Err(RusbmuxError::DeviceNotFound(device_id));
            }

            packet = conn.recv() => {
                let packet = packet?;
                debug!(device_id, port_number, "Received packet from device");

                client_send(&mut client_writer, packet.payload()).await?;
            }

            client_packet = client_read(&mut client_reader, &mut read_buf, conn.sendable_bytes()),
                            if conn.sendable_bytes() > 0
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
            return Ok(buf.split_to(sendable_bytes.min(buf.len())).freeze());
        }

        buf.reserve(CLIENT_BUFF_SIZE);
        let n = client
            .read_buf(buf)
            .await
            .inspect_err(|e| error!(err = ?e, "Failed to read from client"))?;

        if n == 0 {
            return Ok(Bytes::new());
        }
    }
}

pub async fn client_send(
    client: &mut impl AsyncWriting,
    payload: Bytes,
) -> Result<(), RusbmuxError> {
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

pub async fn connect(usbmux_packet: &UsbMuxPacket) -> Result<DeviceConn, RusbmuxError> {
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
        .find(|dev| dev.id() == device_id)
        .ok_or(RusbmuxError::DeviceNotFound(device_id))?;

    let conn = device.connect(port_number).await?;

    drop(connected_devices);

    Ok(conn)
}
