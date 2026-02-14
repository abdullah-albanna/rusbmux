use nusb::descriptors::InterfaceDescriptor;

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
    let current_cfg = dev.active_configuration().unwrap();

    let (intf, intf_cfg_num) = dev
        .configurations()
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

    if intf_cfg_num != current_cfg.configuration_value() {
        println!("found the interface in another config");
        println!(
            "the old config: {}, new config: {}",
            current_cfg.configuration_value(),
            intf_cfg_num
        );

        dev.set_configuration(intf_cfg_num).await.unwrap();
    }

    intf
}
