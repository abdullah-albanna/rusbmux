use std::io::ErrorKind;

use tokio::io::AsyncWriteExt;

use crate::{
    AsyncWriting, ReadWrite,
    handler::{
        connect::handle_connect, device_list::handle_device_list, listen::handle_listen,
        listeners_list::handle_listeners_list, read_pair_record::handle_read_pair_record,
    },
    parser::usbmux::{PayloadMessageType, UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

pub mod connect;
pub mod device_list;
pub mod listen;
pub mod listeners_list;
pub mod read_pair_record;

pub async fn handle_client(mut client: Box<dyn ReadWrite>) {
    loop {
        let usbmux_packet = match UsbMuxPacket::from_reader(&mut client).await {
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

                let payload_msg_type: PayloadMessageType = payload
                    .as_dictionary()
                    .expect("payload was not a dictionay")
                    .get("MessageType")
                    .expect("there was no `MessageType` key in the payload")
                    .as_string()
                    .expect("the `MessageType` was not a string")
                    .try_into()
                    .expect("the `MessageType` is not valid");

                match payload_msg_type {
                    PayloadMessageType::ListDevices => {
                        handle_device_list(&mut client, usbmux_packet.header.tag).await;
                    }

                    PayloadMessageType::Listen => {
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
                        // we don't get usbmux packets once connected
                        //
                        // FIXME: what if the connect failed?
                        handle_connect(client, usbmux_packet).await;
                        break;
                    }
                    _ => unimplemented!("{payload_msg_type:?} is not yet implemented"),
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

    writer
        .write_all(&result_usbmux_packet)
        .await
        .expect("unable to send the listen result");
    writer.flush().await.unwrap();
}
