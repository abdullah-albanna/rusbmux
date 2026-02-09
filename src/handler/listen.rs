use futures_lite::StreamExt;
use nusb::hotplug::HotplugEvent;

use crate::{
    AsyncWriting,
    handler::device_list::create_device_connected_plist_from_deviceinfo,
    parser::usbmux::{UsbMuxHeader, UsbMuxMsgType, UsbMuxPacket, UsbMuxPayload, UsbMuxVersion},
};

use tokio::io::AsyncWriteExt;

pub async fn handle_listen(writer: &mut impl AsyncWriting, tag: u32) {
    let mut devices = nusb::watch_devices().unwrap();

    while let Some(event) = devices.next().await {
        match event {
            HotplugEvent::Connected(device) => {
                let device_plist = create_device_connected_plist_from_deviceinfo(&device);

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let connected_packet = UsbMuxPacket {
                    header: UsbMuxHeader {
                        len: (device_xml.len() + UsbMuxHeader::SIZE) as _,
                        version: UsbMuxVersion::Plist,
                        msg_type: UsbMuxMsgType::MessagePlist,
                        tag,
                    },
                    payload: UsbMuxPayload::Plist(device_plist),
                };

                writer
                    .write_all(&connected_packet.encode())
                    .await
                    .expect("unable to send the listen connect event");
                writer.flush().await.unwrap();
            }
            HotplugEvent::Disconnected(device) => {
                // FIXME: this is so bad
                let id = {
                    #[cfg(target_os = "linux")]
                    let bytes: [u8; std::mem::size_of::<nusb::DeviceId>()] =
                        unsafe { std::mem::transmute(device) };

                    #[cfg(target_os = "macos")]
                    let bytes: [u64; std::mem::size_of::<nusb::DeviceId>()] =
                        unsafe { std::mem::transmute(device) };

                    // the first is busnum
                    bytes[0] as u64
                };

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
                    payload: UsbMuxPayload::Plist(device_plist),
                };

                writer
                    .write_all(&disconnected_packet.encode())
                    .await
                    .expect("unable to send the listen disconnect event");
                writer.flush().await.unwrap();
            }
        }
    }
}
