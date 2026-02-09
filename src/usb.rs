use nusb::descriptors::InterfaceDescriptor;

const APPLE_VID: u16 = 0x5ac;

pub async fn get_apple_device() -> impl Iterator<Item = nusb::DeviceInfo> {
    nusb::list_devices()
        .await
        .unwrap()
        .filter(|dev| dev.vendor_id() == APPLE_VID)
}

pub async fn find_usbmux_interface<'a>(dev: &'a nusb::Device) -> InterfaceDescriptor<'a> {
    // TODO: get the current active configuration, see if the needed interface configuration is
    // different, then set it as the configuration for the device
    for config in dev.configurations() {
        for interface in config.interface_alt_settings() {
            if interface.class() != 255 || interface.subclass() != 254 || interface.protocol() != 2
            {
                continue;
            }

            return interface;
        }
    }
    unreachable!("just to test")
}
