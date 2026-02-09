use crate::{
    AsyncWriting,
    parser::usbmux::{UsbMuxHeader, UsbMuxMsgType, UsbMuxPacket, UsbMuxPayload, UsbMuxVersion},
    usb,
    utils::nusb_speed_to_number,
};

use nusb::Speed;
use tokio::io::AsyncWriteExt;

pub fn create_device_connected_plist(
    id: u8,
    speed: u32,
    device_address: u8,
    product_id: u16,
    serial_number: String,
) -> plist::Value {
    plist_macro::plist!({
        "MessageType": "Attached",
        "DeviceID": id as u16,
        "Properties": {
            "ConnectionSpeed": speed,
            "ConnectionType": "USB",
            "DeviceID": id as u16,
            "LocationID": device_address as u16,
            "ProductID": product_id,
            "SerialNumber": serial_number,
        }
    })
}
pub async fn devices_plist() -> plist::Value {
    let connected_devices = usb::get_apple_device().await;

    // TODO: maybe get the len of the devices, and do `.with_capacity()`
    let mut devices_plist = Vec::new();

    // TODO: the device id should be unique to each device
    for device in connected_devices {
        devices_plist.push(create_device_connected_plist(
            device.busnum(),
            nusb_speed_to_number(device.speed().unwrap_or(Speed::Low)),
            device.device_address(),
            device.product_id(),
            device.serial_number().unwrap_or_default().to_string(),
        ));
    }

    plist_macro::plist!({
        "DeviceList": devices_plist
    })
}

pub async fn handle_device_list(writer: &mut impl AsyncWriting, tag: u32) {
    let devices_plist = devices_plist().await;

    let devices_xml = plist_macro::plist_value_to_xml_bytes(&devices_plist);

    let usbmux_packet = UsbMuxPacket {
        header: UsbMuxHeader {
            len: (devices_xml.len() + UsbMuxHeader::SIZE) as _,
            version: UsbMuxVersion::Plist,
            msg_type: UsbMuxMsgType::MessagePlist,
            tag,
        },
        payload: UsbMuxPayload::Raw(devices_xml),
    };

    writer
        .write_all(&usbmux_packet.encode())
        .await
        .expect("unable to send the packet");
    writer.flush().await.unwrap();
}
