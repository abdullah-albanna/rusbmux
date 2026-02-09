use crate::{
    AsyncWriting, DeviceEvent,
    handler::device_list::create_device_connected_plist,
    parser::usbmux::{UsbMuxHeader, UsbMuxMsgType, UsbMuxPacket, UsbMuxPayload, UsbMuxVersion},
};

use tokio::{io::AsyncWriteExt, sync::broadcast};

pub async fn handle_listen(
    writer: &mut impl AsyncWriting,
    tag: u32,
    event_receiver: &mut broadcast::Receiver<DeviceEvent>,
) {
    while let Ok(event) = event_receiver.recv().await {
        match event {
            DeviceEvent::Attached {
                serial_number,
                id,
                speed,
                product_id,
                device_address,
            } => {
                let device_plist = create_device_connected_plist(
                    id as _,
                    speed,
                    device_address,
                    product_id,
                    serial_number,
                );

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let connected_packet = UsbMuxPacket {
                    header: UsbMuxHeader {
                        len: (device_xml.len() + UsbMuxHeader::SIZE) as _,
                        version: UsbMuxVersion::Plist,
                        msg_type: UsbMuxMsgType::MessagePlist,
                        tag,
                    },
                    payload: UsbMuxPayload::Raw(device_xml),
                };

                if let Err(_e) = writer.write_all(&connected_packet.encode()).await {
                    // eprintln!("unable to send the listen connect event, {e}");
                }

                writer.flush().await.unwrap();
            }
            DeviceEvent::Detached { id } => {
                let device_plist = plist_macro::plist!({
                    "MessageType": "Detached",
                    "DeviceID": id
                });

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let disconnected_packet = UsbMuxPacket {
                    header: UsbMuxHeader {
                        len: (device_xml.len() + UsbMuxHeader::SIZE) as _,
                        version: UsbMuxVersion::Plist,
                        msg_type: UsbMuxMsgType::MessagePlist,
                        tag,
                    },
                    payload: UsbMuxPayload::Raw(device_xml),
                };

                if let Err(_e) = writer.write_all(&disconnected_packet.encode()).await {
                    // eprintln!("unable to send the listen disconnect event");
                }
                writer.flush().await.unwrap();
            }
        }
    }
}
