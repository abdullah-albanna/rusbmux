use nusb::{
    Endpoint,
    descriptors::InterfaceDescriptor,
    transfer::{Bulk, Direction, In, Out},
};
use tokio::io::AsyncWriteExt;

use crate::parser::{
    device_mux::{DeviceMuxPacket, DeviceMuxPayload, DeviceMuxVersion},
    device_mux_builder::DeviceMuxPacketBuilder,
};

pub const APPLE_VID: u16 = 0x5ac;

pub async fn get_apple_device() -> impl Iterator<Item = nusb::DeviceInfo> {
    nusb::list_devices()
        .await
        .unwrap()
        .filter(|dev| dev.vendor_id() == APPLE_VID)
}

pub const APPLE_USBMUX_CLASS: u8 = 255;
pub const APPLE_USBMUX_SUBCLASS: u8 = 254;
pub const APPLE_USBMUX_PROTOCOL: u8 = 2;

pub async fn get_usbmux_interface<'a>(dev: &'a nusb::Device) -> InterfaceDescriptor<'a> {
    let current_cfg = dev
        .active_configuration()
        .map_or(0, |c| c.configuration_value());

    let (intf, intf_cfg_num) = dev
        .configurations()
        // search from the bottom up
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|cfg| (cfg.configuration_value(), cfg.interface_alt_settings()))
        .filter_map(|(cfg_num, intfs)| {
            for intf in intfs {
                if intf.class() == APPLE_USBMUX_CLASS
                    && intf.subclass() == APPLE_USBMUX_SUBCLASS
                    && intf.protocol() == APPLE_USBMUX_PROTOCOL
                {
                    return Some((intf, cfg_num));
                }
            }
            None
        })
        .next()
        .expect("there was no usbmux interface available");

    // zero means it doesn't have any active configuration
    if intf_cfg_num != current_cfg || current_cfg == 0 {
        println!("found the interface in another config");
        println!(
            "the old config: {}, new config: {}",
            current_cfg, intf_cfg_num
        );

        // TODO: maybe don't search for it again
        let cfg = dev
            .configurations()
            .find(|c| c.configuration_value() == intf_cfg_num)
            .unwrap();

        // make sure to detach any interfaces before setting the new configuration
        for intf in cfg.interface_alt_settings() {
            if intf.alternate_setting() != 0 {
                continue;
            }

            dev.detach_kernel_driver(intf.interface_number()).ok();
        }

        dev.set_configuration(intf_cfg_num).await.unwrap();
    }

    intf
}

pub async fn get_usb_endpoints<'a>(
    dev: &'a nusb::Device,
    interface_descriptor: &InterfaceDescriptor<'a>,
) -> (Endpoint<Bulk, Out>, Endpoint<Bulk, In>) {
    let mut endpoint_out: Option<Endpoint<Bulk, Out>> = None;
    let mut endpoint_in: Option<Endpoint<Bulk, In>> = None;

    let intf = dev
        .claim_interface(interface_descriptor.interface_number())
        .await
        .expect("unable to claim the interface");

    for endpoint in interface_descriptor.endpoints() {
        match endpoint.direction() {
            Direction::Out => {
                endpoint_out = Some(
                    intf.endpoint(endpoint.address())
                        .expect("unable to get the bulk out endpoint"),
                );
            }

            Direction::In => {
                endpoint_in = Some(
                    intf.endpoint(endpoint.address())
                        .expect("unable to get the bulk in endpoint"),
                );
            }
        }
    }

    (
        endpoint_out.expect("there was no bulk out endpoint"),
        endpoint_in.expect("there was no bulk in endpoint"),
    )
}

pub async fn get_device_speaking_version(dev: &nusb::DeviceInfo) -> DeviceMuxVersion {
    let dev = dev.open().await.expect("unable to open the device");
    let interface = get_usbmux_interface(&dev).await;
    let (end_out, end_in) = get_usb_endpoints(&dev, &interface).await;
    let mut end_out = end_out.writer(512);
    let mut end_in = end_in.reader(512);

    let packet = DeviceMuxPacketBuilder::new()
        .header_version()
        .payload_version(2, 0)
        .build()
        .expect("unable to build the get version packet");

    end_out.write_all(&packet.encode()).await.unwrap();
    end_out.flush().await.unwrap();

    let res_packet = DeviceMuxPacket::parse(&mut end_in).await;

    match res_packet.payload {
        DeviceMuxPayload::Version(v) => v,
        _ => panic!("response wasn't a version packet, packet: {res_packet:#?}"),
    }
}
