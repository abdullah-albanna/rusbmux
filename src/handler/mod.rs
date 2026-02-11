use std::io::ErrorKind;

use crate::{
    ReadWrite,
    handler::{
        device_list::handle_device_list,
        listen::{handle_listen, send_result_okay},
        listeners_list::handle_listeners_list,
        read_pair_record::handle_read_pair_record,
    },
    parser::usbmux::{PayloadMessageType, UsbMuxMsgType, UsbMuxPacket},
};

pub mod device_list;
pub mod device_watcher;
pub mod listen;
pub mod listeners_list;
pub mod read_pair_record;

pub async fn handle_client(client: &mut impl ReadWrite) {
    loop {
        let usbmux_packet = match UsbMuxPacket::parse(client).await {
            Ok(p) => p,

            // client closed connection
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                break;
            }

            Err(e) => {
                eprintln!("error while reading the usbmux packet, error: {e}");
                continue;
            }
        };

        println!("{usbmux_packet:#?}");

        match usbmux_packet.header.msg_type {
            UsbMuxMsgType::MessagePlist => {
                let payload = usbmux_packet
                    .payload
                    .clone()
                    .as_plist()
                    .expect("shouldn't fail");

                let dict_payload = payload
                    .as_dictionary()
                    .expect("payload was not a dictionay");

                let payload_msg_type: PayloadMessageType = dict_payload
                    .get("MessageType")
                    .expect("there was no `MessageType` key in the payload")
                    .as_string()
                    .expect("the `MessageType` was not a string")
                    .try_into()
                    .expect("the `MessageType` is not valid");

                match payload_msg_type {
                    PayloadMessageType::ListDevices => {
                        handle_device_list(client, usbmux_packet.header.tag).await
                    }

                    PayloadMessageType::Listen => {
                        send_result_okay(client, usbmux_packet.header.tag).await;
                        handle_listen(client, usbmux_packet.header.tag).await;
                    }
                    PayloadMessageType::ListListeners => {
                        handle_listeners_list(client, usbmux_packet.header.tag).await;
                    }
                    PayloadMessageType::ReadPairRecord => {
                        handle_read_pair_record(client, &usbmux_packet).await;
                    }
                    _ => unimplemented!("{payload_msg_type:?} is not yet implemented"),
                }
            }
            _ => unimplemented!("{:?} is not yet implemented", usbmux_packet.header.msg_type),
        }
    }
}
