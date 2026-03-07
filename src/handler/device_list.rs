use crate::{
    AsyncWriting,
    device::CONNECTED_DEVICES,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
    utils::nusb_speed_to_number,
};

use nusb::Speed;
use tokio::io::AsyncWriteExt;

#[must_use]
pub fn create_device_connected_plist(
    id: u64,
    speed: u64,
    location_id: u32,
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
            "LocationID": location_id,
            "ProductID": product_id,
            "SerialNumber": serial_number,
        }
    })
}
pub async fn devices_plist() -> plist::Value {
    let connected_devices = &*CONNECTED_DEVICES.read().await;

    let mut devices_plist = Vec::with_capacity(connected_devices.len());

    for device in connected_devices {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let location_id = (device.info.busnum() as u32) << 16 | device.info.device_address() as u32;

        #[cfg(target_os = "macos")]
        let location_id = device.info.location_id();

        let speed = nusb_speed_to_number(device.info.speed().unwrap_or(Speed::Low));

        devices_plist.push(create_device_connected_plist(
            device.id,
            speed,
            location_id,
            device.info.product_id(),
            device.info.serial_number().unwrap_or_default().to_string(),
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
