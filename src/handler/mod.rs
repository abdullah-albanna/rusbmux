use std::{io::ErrorKind, ops::ControlFlow};

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, trace, warn};

use crate::{
    AsyncWriting, ReadWrite,
    error::{ParseError, RusbmuxError},
    handler::{
        connect::handle_connect, delete_pair_record::handle_delete_pair_record,
        device_list::handle_device_list, listen::handle_listen,
        listeners_list::handle_listeners_list, read_buid::handle_read_buid,
        read_pair_record::handle_read_pair_record, save_pair_record::handle_save_pair_record,
    },
    parser::usbmux::{PayloadMessageType, UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

pub mod connect;
pub mod delete_pair_record;
pub mod device_list;
pub mod listen;
pub mod listeners_list;
pub mod read_buid;
pub mod read_pair_record;
pub mod save_pair_record;

#[cfg(target_os = "macos")]
pub const CONFIG_PATH: &str = "/var/db";

#[cfg(not(target_os = "macos"))]
pub const CONFIG_PATH: &str = "/var/lib";

pub async fn handle_client(mut client: Box<dyn ReadWrite>) {
    loop {
        let usbmux_packet = match UsbMuxPacket::from_reader(&mut client).await {
            Ok(p) => p,

            // client closed connection
            Err(ParseError::IO(e))
                if matches!(
                    e.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
                ) =>
            {
                warn!("Client disconnected, closing");
                break;
            }

            Err(e) => {
                error!( err = ?e, "Failed to read usbmux packet");
                continue;
            }
        };

        let tag = usbmux_packet.header.tag;

        debug!(
            tag,
            msg_type = ?usbmux_packet.header.msg_type,
            "Received usbmux packet"
        );

        match handle_message(&mut client, usbmux_packet).await {
            // comes from the ones that transforms the connection (Connect, Listen), because you're
            // not supposed to do anything else if those failed
            Ok(ControlFlow::Break(())) | Err(RusbmuxError::DeviceNotFound(_)) => {
                return;
            }

            // it's an error, but that doesn't mean to close the connection
            Err(e) => {
                error!(err = ?e, tag, "Handler failed");
                continue;
            }

            Ok(ControlFlow::Continue(())) => continue,
        }
    }
}

pub async fn handle_message(
    client: &mut impl ReadWrite,
    usbmux_packet: UsbMuxPacket,
) -> Result<ControlFlow<()>, RusbmuxError> {
    let tag = usbmux_packet.header.tag;

    match usbmux_packet.header.msg_type {
        UsbMuxMsgType::MessagePlist => {
            let payload = usbmux_packet.payload.as_plist().expect("shouldn't fail");

            let payload_msg_type: PayloadMessageType = payload
                .as_dictionary()
                .ok_or(RusbmuxError::UnexpectedPacket(
                    "Expected a packet with a dictionary plist payload".to_string(),
                ))?
                .get("MessageType")
                .ok_or(RusbmuxError::ValueNotFound("MessageType"))?
                .as_string()
                .ok_or(RusbmuxError::InvalidData("MessageType is not a string"))?
                .try_into()
                .map_err(|_| RusbmuxError::InvalidData("MessageType is not valid"))?;

            debug!(
                tag,
                payload_type = ?payload_msg_type,
                "Dispatching request"
            );

            match payload_msg_type {
                PayloadMessageType::ListDevices => {
                    handle_device_list(client, usbmux_packet.header.tag).await?;
                }

                PayloadMessageType::Listen => {
                    info!(tag, "Client entered listen mode");
                    handle_listen(client, usbmux_packet.header.tag).await?;

                    info!(tag, "Listener handed off");
                    return Ok(ControlFlow::Break(()));
                }
                PayloadMessageType::ListListeners => {
                    handle_listeners_list(client, usbmux_packet.header.tag).await?;
                }
                PayloadMessageType::ReadPairRecord => {
                    handle_read_pair_record(client, &usbmux_packet).await?;
                }
                PayloadMessageType::Connect => {
                    info!(tag, "Client entered connect mode");

                    handle_connect(client, usbmux_packet).await?;

                    info!(tag, "Connection handed off");
                    return Ok(ControlFlow::Break(()));
                }
                PayloadMessageType::ReadBUID => {
                    handle_read_buid(client, &usbmux_packet).await?;
                }
                PayloadMessageType::SavePairRecord => {
                    handle_save_pair_record(client, &usbmux_packet).await?;
                }
                PayloadMessageType::DeletePairRecord => {
                    handle_delete_pair_record(client, &usbmux_packet).await?;
                }
            }
        }
        _ => unimplemented!("{:?} is not yet implemented", usbmux_packet.header.msg_type),
    }

    Ok(ControlFlow::Continue(()))
}

#[repr(u16)]
pub enum ResultCode {
    OK = 0,
    BadCommand = 1,
    BadDeviceOrNoSuchFile = 2,
    ConnectionRefused = 3,
    BadVesion = 6,
    InvalidInput = 22,
}

pub async fn send_result(
    writer: &mut impl AsyncWriting,
    code: ResultCode,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let result_payload = plist_macro::plist!({
        "MessageType": "Result",
        "Number": (code as u16)
    });

    let result_payload_xml = plist_macro::plist_value_to_xml_bytes(&result_payload);

    let result_usbmux_packet = UsbMuxPacket::encode_from(
        result_payload_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );
    writer
        .write_all(&result_usbmux_packet)
        .await
        .inspect_err(|e| error!(tag, err = ?e, "Failed to send OKAY"))?;

    writer
        .flush()
        .await
        .inspect_err(|e| error!(tag, err = ?e, "Failed to flush OKAY response"))?;

    trace!(tag, "Sent OKAY response");

    Ok(())
}

pub async fn create_lockdown_dir() -> Result<(), RusbmuxError> {
    let path = format!("{CONFIG_PATH}/lockdown");

    tokio::fs::create_dir_all(&path)
        .await
        .inspect_err(|e| error!(path, e = ?e, "Failed to create the lockdown folder"))?;

    Ok(())
}
