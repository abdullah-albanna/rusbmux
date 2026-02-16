use crate::{
    AsyncWriting,
    device_watcher::CONNECTED_DEVICES,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
    utils::nusb_speed_to_number,
};

use nusb::Speed;
use tokio::io::AsyncWriteExt;

pub fn create_device_connected_plist(
    id: u32,
    speed: u32,
    device_address: u8,
    product_id: u16,
    serial_number: String,
) -> plist::Value {
    plist_macro::plist!({
        "MessageType": "Attached",
        "DeviceID": id,
        "Properties": {
            "ConnectionSpeed": speed,
            "ConnectionType": "USB",
            "DeviceID": id,
            "LocationID": device_address as u16,
            "ProductID": product_id,
            "SerialNumber": serial_number,
        }
    })
}
pub async fn devices_plist() -> plist::Value {
    let connected_devices = &*CONNECTED_DEVICES.read().await;

    println!("devices: {connected_devices:#?}");

    let mut devices_plist = Vec::with_capacity(connected_devices.len());

    for (device, id) in connected_devices {
        devices_plist.push(create_device_connected_plist(
            *id,
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

    let usbmux_packet = UsbMuxPacket::encode_from(
        devices_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );

    writer
        .write_all(&usbmux_packet)
        .await
        .expect("unable to send the packet");
    writer.flush().await.unwrap();
}
