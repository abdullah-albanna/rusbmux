use std::io::ErrorKind;

use crate::{
    DeviceEvent, ReadWrite,
    handler::{device_list::handle_device_list, listen::handle_listen},
    parser::usbmux::{
        PayloadMessageType, UsbMuxHeader, UsbMuxMsgType, UsbMuxPacket, UsbMuxPayload, UsbMuxVersion,
    },
};
use tokio::{io::AsyncWriteExt, sync::broadcast};

pub mod device_list;
pub mod device_watcher;
pub mod listen;

pub async fn handle_client(
    client: &mut impl ReadWrite,
    mut event_receiver: broadcast::Receiver<DeviceEvent>,
) {
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
                let payload = usbmux_packet.payload.as_plist().expect("shouldn't fail");

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
                        let result_payload = plist_macro::plist!({
                            "MessageType": "Result",
                            "Number": 0 // 0 means okay
                        });

                        let result_payload_xml =
                            plist_macro::plist_value_to_xml_bytes(&result_payload);

                        let result_packet = UsbMuxPacket {
                            header: UsbMuxHeader {
                                len: (result_payload_xml.len() + UsbMuxHeader::SIZE) as _,
                                version: UsbMuxVersion::Plist,
                                msg_type: UsbMuxMsgType::MessagePlist,
                                tag: usbmux_packet.header.tag,
                            },
                            payload: UsbMuxPayload::Raw(result_payload_xml),
                        };

                        client
                            .write_all(&result_packet.encode())
                            .await
                            .expect("unable to send the listen result");
                        client.flush().await.unwrap();

                        handle_listen(client, usbmux_packet.header.tag, &mut event_receiver).await;
                    }
                    _ => unimplemented!("{payload_msg_type:?} is not yet implemented"),
                }
            }
            _ => unimplemented!("{:?} is not yet implemented", usbmux_packet.header.msg_type),
        }
    }
}
