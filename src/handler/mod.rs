use crate::{
    AsyncWriting,
    parser::usbmux::{PayloadMessageType, UsbMuxMsgType, UsbMuxPacket},
};

pub mod device_list;

pub async fn handle_usbmux(usbmux_packet: UsbMuxPacket, writer: &mut impl AsyncWriting) {
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
                    device_list::handle_device_list(writer, usbmux_packet.header.tag).await
                }
                _ => unimplemented!("{payload_msg_type:?} is not yet implemented"),
            }
        }
        _ => unimplemented!("{:?} is not yet implemented", usbmux_packet.header.msg_type),
    }
}
