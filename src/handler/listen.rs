use crate::{
    AsyncWriting,
    device::{DeviceEvent, HOTPLUG_EVENT_TX},
    handler::device_list::create_device_connected_plist,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

use tokio::io::AsyncWriteExt;

pub async fn handle_listen(writer: &mut impl AsyncWriting, tag: u32) {
    let mut event_receiver = HOTPLUG_EVENT_TX
        .get()
        .expect("the device watcher hasen't been initlized (very unlikely)")
        .subscribe();

    while let Ok(event) = event_receiver.recv().await {
        match event {
            DeviceEvent::Attached {
                serial_number,
                id,
                speed,
                product_id,
                location_id,
            } => {
                let device_plist = create_device_connected_plist(
                    id as _,
                    speed,
                    location_id,
                    product_id,
                    serial_number,
                );

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let connected_packet = UsbMuxPacket::encode_from(
                    device_xml,
                    UsbMuxVersion::Plist,
                    UsbMuxMsgType::MessagePlist,
                    tag,
                );

                if let Err(_e) = writer.write_all(&connected_packet).await {
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

                let disconnected_packet = UsbMuxPacket::encode_from(
                    device_xml,
                    UsbMuxVersion::Plist,
                    UsbMuxMsgType::MessagePlist,
                    tag,
                );
                if let Err(_e) = writer.write_all(&disconnected_packet).await {
                    // eprintln!("unable to send the listen disconnect event");
                }
                writer.flush().await.unwrap();
            }
        }
    }
}
