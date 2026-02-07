use crate::{
    AsyncWriting,
    parser::usbmux::{UsbMuxHeader, UsbMuxMsgType, UsbMuxPacket, UsbMuxPayload, UsbMuxVersion},
    usb,
};

use nusb::Speed;
use tokio::io::AsyncWriteExt;

pub async fn create_device_attached_plist() -> plist::Value {
    let connected_devices = usb::get_apple_device().await;

    // TODO: maybe get the len of the devices, and do `.with_capacity()`
    let mut devices_plist = Vec::new();

    for device in connected_devices {
        let id = device.busnum() as u32;
        let speed = match device.speed().unwrap_or(Speed::Low) {
            Speed::Low => 1,
            Speed::Full => 12,
            Speed::High => 480,
            Speed::Super => 5000,
            Speed::SuperPlus => 10000,
            _ => unreachable!("heh?"),
        };

        devices_plist.push(plist_macro::plist!({
            "MessageType": "Attached",
            "DeviceID": id,
            "Properties": {
                "ConnectionSpeed": speed,
                "ConnectionType": "USB",
                "DeviceID": id,
                "LocationID": device.device_address() as u32,
                "ProductID": device.product_id(),
                "SerialNumber": device.serial_number().unwrap(),
            }
        }));
    }

    plist_macro::plist!({
        "DeviceList": devices_plist
    })
}

pub async fn handle_device_list(writer: &mut impl AsyncWriting, tag: u32) {
    let devices_plist = create_device_attached_plist().await;

    // FIXME: I convert this twice, one to get the len, and one to encode
    let device_xml = plist_macro::plist_value_to_xml_bytes(&devices_plist);

    let usbmux_packet = UsbMuxPacket {
        header: UsbMuxHeader {
            len: (device_xml.len() + UsbMuxHeader::SIZE) as _,
            version: UsbMuxVersion::Plist,
            msg_type: UsbMuxMsgType::MessagePlist,
            tag,
        },
        payload: UsbMuxPayload::Plist(devices_plist),
    };

    writer
        .write_all(&usbmux_packet.encode())
        .await
        .expect("unable to send the packet");
}
