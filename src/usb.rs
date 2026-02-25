use nusb::{
    Endpoint,
    descriptors::InterfaceDescriptor,
    transfer::{Bulk, Direction, In, Out},
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

pub async fn get_usbmux_interface(dev: &nusb::Device) -> InterfaceDescriptor<'_> {
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
        .find_map(|(cfg_num, intfs)| {
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
        .expect("there was no usbmux interface available");

    // zero means it doesn't have any active configuration
    if intf_cfg_num != current_cfg || current_cfg == 0 {
        println!("found the interface in another config");
        println!("the old config: {current_cfg}, new config: {intf_cfg_num}",);

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
