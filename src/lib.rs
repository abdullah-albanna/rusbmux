use std::{io::Write, os::unix::net::UnixStream};

use crate::types::{
    UsbMuxHeader,
    usbmux_header::{UsbMuxMsgType, UsbMuxVersion},
};

pub mod types;
pub mod usb;

pub fn create_device_attached_plist() -> plist::Value {
    plist_macro::plist!([{
        "MessageType": "Attached",
        "DeviceID": 67676767,
        "Properties": {
            "ConnectionSpeed": 67,
            "ConnectionType": "USB",
            "DeviceID": 67676767,
            "LocationID": 67,
            "ProductID": 67,
            "SerialNumber": "ksi forehead is so big",
        }
    }])
}

pub async fn send_plist(dev: &mut UnixStream, data: plist::Value, tag: u32) {
    let mut xml = plist_macro::plist_value_to_xml_bytes(&data);
    let xml_size = xml.len();

    let h = UsbMuxHeader {
        len: (xml_size + UsbMuxHeader::SIZE) as _,
        version: UsbMuxVersion::Plist,
        msg_type: UsbMuxMsgType::MessagePlist,
        tag,
    };

    let mut h = bytemuck::bytes_of(&h).to_vec();

    h.append(&mut xml);

    dev.write_all(&h).unwrap();
}
pub async fn send_device_list(dev: &mut UnixStream, tag: u32) {
    let device = create_device_attached_plist();
    send_plist(
        dev,
        plist_macro::plist!({
            "DeviceList": device
        }),
        tag,
    )
    .await
}
