use std::sync::Arc;

use crate::{
    AsyncReading, AsyncWriting, ReadWrite,
    conn::{DeviceConn, NetworkDeviceConn, UsbDeviceConn},
    error::RusbmuxError,
    handler::send_result,
    watcher::CONNECTED_DEVICES,
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info, trace};

use super::ResultCode;

const CLIENT_BUFF_SIZE: usize = 128 * 1024;

pub async fn handle_connect(
    mut client: Box<dyn ReadWrite>,
    device_id: u64,
    port_number: u16,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let conn = match connect(device_id, port_number, tag).await {
        Ok(c) => c,
        Err(e) => {
            match e {
                RusbmuxError::DeviceNotFound(_) | RusbmuxError::RanOutofSourcePort => {
                    send_result(&mut client, ResultCode::BadDeviceOrNoSuchFile, tag).await?;
                }

                _ => {
                    send_result(&mut client, ResultCode::ConnectionRefused, tag).await?;
                }
            }
            return Err(e);
        }
    };

    send_result(&mut client, ResultCode::OK, tag).await?;

    match conn {
        DeviceConn::Usb(conn) => handle_usb_device_connect(client, conn).await?,
        DeviceConn::Network(conn) => handle_network_device_connect(client, conn).await?,
    }

    Ok(())
}

pub async fn handle_network_device_connect(
    mut client: Box<dyn ReadWrite>,
    mut conn: NetworkDeviceConn,
) -> Result<(), RusbmuxError> {
    let device_id = conn.device_id;
    let port_number = conn.destination_port;

    let canceler = conn.device_canceler.clone();

    tokio::select! {
        res = tokio::io::copy_bidirectional_with_sizes(
            &mut conn.stream,
            &mut client,
            CLIENT_BUFF_SIZE,
            CLIENT_BUFF_SIZE
        ) => {
            res?;
            Ok(())
        }

        _ = canceler.cancelled() => {
            debug!(device_id, port_number, "Shutting down connection");
            Ok(())
        }
    }
}

pub async fn handle_usb_device_connect(
    client: Box<dyn ReadWrite>,
    conn: Arc<UsbDeviceConn>,
) -> Result<(), RusbmuxError> {
    let device_id = conn.device_core.id;
    let port_number = conn.destination_port;

    let mut read_buf = BytesMut::with_capacity(CLIENT_BUFF_SIZE);
    let (mut client_reader, mut client_writer) = tokio::io::split(client);

    loop {
        tokio::select! {
            _ = conn.wait_shutdown() => {
                debug!(device_id, port_number, "Device is shutting down");
                return Ok(());
            }

            packet = conn.recv() => {
                let packet = packet?;
                debug!(device_id, port_number, "Received packet from device");

                client_send(&mut client_writer, packet.payload.encode()).await?;
            }

            client_packet = client_read(&mut client_reader, &mut read_buf, conn.get_sendable_bytes()),
                            if conn.get_sendable_bytes() > 0
            => {
                let client_packet = client_packet?;

                if client_packet.is_empty() {
                    info!(device_id, port_number, "Client disconnected");
                    conn.close().await?;
                    return Ok(());
                }

                debug!(device_id, port_number, "Processing client packet");

                conn.send_bytes(client_packet.freeze()).await?;
            }
        };
    }
}

pub async fn client_read(
    client: &mut dyn AsyncReading,
    buf: &mut BytesMut,
    sendable_bytes: usize,
) -> Result<BytesMut, RusbmuxError> {
    if !buf.is_empty() {
        return Ok(buf.split_to(sendable_bytes.min(buf.len())));
    }

    if !buf.try_reclaim(CLIENT_BUFF_SIZE) || buf.capacity() == 0 {
        buf.reserve(sendable_bytes);
    }

    client.read_buf(buf).await.inspect_err(|e| {
        if !crate::utils::is_disconnect_io(e) {
            error!(err = ?e, "Failed to read from client");
        }
    })?;

    Ok(buf.split_to(sendable_bytes.min(buf.len())))
}

pub async fn client_send(
    client: &mut dyn AsyncWriting,
    payload: Bytes,
) -> Result<(), RusbmuxError> {
    trace!(len = payload.len(), "Sending packet to client");

    client.write_all(&payload).await.inspect_err(|e| {
        if !crate::utils::is_disconnect_io(e) {
            error!(err = ?e, "Failed to write packet to client")
        }
    })?;

    Ok(())
}

pub async fn connect(
    device_id: u64,
    port_number: u16,
    tag: u32,
) -> Result<DeviceConn, RusbmuxError> {
    info!(device_id, port_number, tag, "Client connecting");

    let device = CONNECTED_DEVICES
        .get(&device_id)
        .ok_or(RusbmuxError::DeviceNotFound(device_id))?;

    let conn = device.connect(port_number).await?;

    Ok(conn)
}
