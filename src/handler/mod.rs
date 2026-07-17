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
pub const LOCKDOWN_PATH: &str = "/var/db/lockdown";

#[cfg(target_os = "linux")]
pub const LOCKDOWN_PATH: &str = "/var/lib/lockdown";

#[cfg(windows)]
pub const LOCKDOWN_PATH: &str = "C:\\ProgramData\\Apple\\Lockdown";

pub enum HandlerError {
    Fatal {
        error: RusbmuxError,
        request: Option<PayloadMessageType>,
    },
    NonFatal {
        error: RusbmuxError,
        request: Option<PayloadMessageType>,
    },
}

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
            Ok(ControlFlow::Break(())) => {
                return;
            }

            Err(HandlerError::Fatal { error, request }) => {
                if crate::utils::is_disconnect(&error) {
                    debug!(tag, ?request, "client disconnected");
                    return;
                }

                error!(tag, ?request, err = ?error, "Handler failed");
                return;
            }

            Err(HandlerError::NonFatal { error, request }) => {
                // if the client disconnected, then there's no reason to continue
                if crate::utils::is_disconnect(&error) {
                    debug!(tag, ?request, "client disconnected");
                    return;
                }

                // it's an error, but that doesn't mean to close the connection
                error!(tag, ?request, err = ?error, "Handler failed");
                continue;
            }

            Ok(ControlFlow::Continue(())) => continue,
        }
    }
}

pub async fn handle_message(
    client: &mut Box<dyn ReadWrite>,
    usbmux_packet: UsbMuxPacket,
) -> Result<ControlFlow<()>, HandlerError> {
    let tag = usbmux_packet.header.tag;

    let classify = |error: RusbmuxError, request: Option<PayloadMessageType>| {
        if matches!(
            request,
            Some(PayloadMessageType::Connect | PayloadMessageType::Listen)
        ) {
            HandlerError::Fatal { error, request }
        } else {
            HandlerError::NonFatal { error, request }
        }
    };

    match usbmux_packet.header.msg_type {
        UsbMuxMsgType::MessagePlist => {
            // TODO: implement binary payload

            // TODO: send back badcommand if not plist
            let payload = usbmux_packet.payload.as_plist().ok_or_else(|| {
                classify(
                    RusbmuxError::UnexpectedPacket("expected plist payload".to_string()),
                    None,
                )
            })?;

            debug!(
                "Received payload: {}",
                plist_macro::pretty_print_plist(payload)
            );

            let payload_msg_type: PayloadMessageType = payload
                .as_dictionary()
                .ok_or_else(|| {
                    classify(
                        RusbmuxError::UnexpectedPacket(
                            "Expected a packet with a dictionary plist payload".to_string(),
                        ),
                        None,
                    )
                })?
                .get("MessageType")
                .ok_or_else(|| classify(RusbmuxError::ValueNotFound("MessageType"), None))?
                .as_string()
                .ok_or_else(|| {
                    classify(
                        RusbmuxError::InvalidData("MessageType is not a string"),
                        None,
                    )
                })?
                .try_into()
                .map_err(|_| {
                    classify(RusbmuxError::InvalidData("MessageType is not valid"), None)
                })?;

            debug!(
                tag,
                payload_type = ?payload_msg_type,
                "Dispatching request"
            );

            match payload_msg_type {
                PayloadMessageType::ListDevices => {
                    handle_device_list(client, usbmux_packet.header.tag)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;
                }

                PayloadMessageType::Listen => {
                    info!(tag, "Client entered listen mode");
                    handle_listen(client, usbmux_packet.header.tag)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;

                    info!(tag, "Listener handed off");
                    return Ok(ControlFlow::Break(()));
                }
                PayloadMessageType::ListListeners => {
                    handle_listeners_list(client, usbmux_packet.header.tag)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;
                }
                PayloadMessageType::ReadPairRecord => {
                    handle_read_pair_record(client, &usbmux_packet)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;
                }
                PayloadMessageType::Connect => {
                    info!(tag, "Client entered connect mode");

                    // HACK:
                    let client = std::mem::replace(client, Box::new(std::io::Cursor::new(vec![])));
                    handle_connect(client, usbmux_packet)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;

                    info!(tag, "Connection handed off");
                    return Ok(ControlFlow::Break(()));
                }
                PayloadMessageType::ReadBUID => {
                    handle_read_buid(client, &usbmux_packet)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;
                }
                PayloadMessageType::SavePairRecord => {
                    handle_save_pair_record(client, &usbmux_packet)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;
                }
                PayloadMessageType::DeletePairRecord => {
                    handle_delete_pair_record(client, &usbmux_packet)
                        .await
                        .map_err(|e| classify(e, Some(payload_msg_type)))?;
                }
            }
        }
        // TODO: is this branch readable? if so then send badcommand
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
    BadVersion = 6,
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
        .inspect_err(|e| {
            if !crate::utils::is_disconnect_io(e) {
                error!(tag, err = ?e, "Failed to send OKAY")
            }
        })?;

    trace!(tag, "Sent OKAY response");

    Ok(())
}

pub async fn create_lockdown_dir() -> Result<(), RusbmuxError> {
    tokio::fs::create_dir_all(LOCKDOWN_PATH)
        .await
        .inspect_err(|e| error!(LOCKDOWN_PATH, e = ?e, "Failed to create the lockdown folder"))?;

    Ok(())
}
