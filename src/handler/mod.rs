use std::io::ErrorKind;

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, trace};

use crate::{
    AsyncWriting, ReadWrite,
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
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                info!("Client disconnected (EOF)");
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

        match usbmux_packet.header.msg_type {
            UsbMuxMsgType::MessagePlist => {
                let payload = usbmux_packet.payload.as_plist().expect("shouldn't fail");

                let payload_msg_type: PayloadMessageType = match payload
                    .as_dictionary()
                    .and_then(|d| d.get("MessageType"))
                    .and_then(|v| v.as_string())
                    .and_then(|s| s.try_into().ok())
                {
                    Some(t) => t,
                    None => {
                        error!(tag, "Invalid or missing MessageType");
                        continue;
                    }
                };

                debug!(

                    tag,
                    payload_type = ?payload_msg_type,
                    "Dispatching request"
                );

                match payload_msg_type {
                    PayloadMessageType::ListDevices => {
                        handle_device_list(&mut client, usbmux_packet.header.tag).await;
                    }

                    PayloadMessageType::Listen => {
                        info!(tag, "Client entered listen mode");
                        send_result_okay(&mut client, usbmux_packet.header.tag).await;
                        handle_listen(&mut client, usbmux_packet.header.tag).await;
                    }
                    PayloadMessageType::ListListeners => {
                        handle_listeners_list(&mut client, usbmux_packet.header.tag).await;
                    }
                    PayloadMessageType::ReadPairRecord => {
                        handle_read_pair_record(&mut client, &usbmux_packet).await;
                    }
                    PayloadMessageType::Connect => {
                        info!(tag, "Client requested connect");

                        // we don't get usbmux packets once connected
                        //
                        // FIXME: what if the connect failed?
                        handle_connect(client, usbmux_packet).await;

                        info!(tag, "Connection handed off");
                        break;
                    }
                    PayloadMessageType::ReadBUID => {
                        handle_read_buid(&mut client, &usbmux_packet).await;
                    }
                    PayloadMessageType::SavePairRecord => {
                        handle_save_pair_record(&mut client, &usbmux_packet).await;
                    }
                    PayloadMessageType::DeletePairRecord => {
                        handle_delete_pair_record(&mut client, &usbmux_packet).await;
                    }
                }
            }
            _ => unimplemented!("{:?} is not yet implemented", usbmux_packet.header.msg_type),
        }
    }
}

pub async fn send_result_okay(writer: &mut impl AsyncWriting, tag: u32) {
    let result_payload = plist_macro::plist!({
        "MessageType": "Result",
        "Number": 0 // 0 means okay
    });

    let result_payload_xml = plist_macro::plist_value_to_xml_bytes(&result_payload);

    let result_usbmux_packet = UsbMuxPacket::encode_from(
        result_payload_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );

    if let Err(e) = writer.write_all(&result_usbmux_packet).await {
        error!(tag, err = ?e, "Failed to send OKAY");
    }
    if let Err(e) = writer.flush().await {
        error!(tag, err = ?e, "Failed to flush OKAY response");
    }

    trace!(tag, "Sent OKAY response");
}
