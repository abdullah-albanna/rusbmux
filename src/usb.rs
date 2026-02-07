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

// const APPLE_VENDOR_GET_MODE: u8 = 0x45;
// const APPLE_VENDOR_SET_MODE: u8 = 0x52;
//
// /// usbmuxd/src/usb.c:918
// ///
// /// On top of configurations, Apple have multiple "modes" for devices, namely:
// /// 1: An "initial" mode with 4 configurations
// /// 2: "Valeria" mode, where configuration 5 is included with interface for
// /// H.265 video capture (activated when recording screen with QuickTime in
// /// macOS) 3: "CDC NCM" mode, where configuration 5 is included with interface
// /// for Ethernet/USB (activated using internet-sharing feature in macOS)
// /// Request current mode asynchroniously
// ///
// ///
// /// Response is 3:3:3:0 for initial mode, 5:3:3:0 otherwise.
// pub async fn get_vendor_specific_mode(dev: &nusb::Device) -> Vec<u8> {
//     dev.control_in(
//         ControlIn {
//             control_type: nusb::transfer::ControlType::Vendor,
//             recipient: nusb::transfer::Recipient::Device,
//             request: APPLE_VENDOR_GET_MODE,
//             value: 0,
//             index: 0,
//             length: 4,
//         },
//         std::time::Duration::from_secs(1),
//     )
//     .await
//     .inspect(|data| println!("get mode: {data:?}"))
//     .unwrap()
// }
//
// pub async fn set_vendor_specific_mode(dev: &nusb::Device) -> Vec<u8> {
//     dev.control_in(
//         ControlIn {
//             control_type: nusb::transfer::ControlType::Vendor,
//             recipient: nusb::transfer::Recipient::Device,
//             request: APPLE_VENDOR_SET_MODE,
//             value: 0,
//             index: 1,
//             length: 1,
//         },
//         std::time::Duration::from_secs(1),
//     )
//     .await
//     .inspect(|data| println!("get mode: {data:?}"))
//     .unwrap()
// }
//
// pub enum CurrentMode {
//     Undertermined,
//     Initial,
//     Valeria,
//     CDCNCM,
//     USBEthWithCDCNCM,
//     CDCNCMDirect,
// }
// pub async fn guess_mode(dev: &nusb::Device) -> CurrentMode {
//     let num_configs = dev.configurations().count();
//
//     if num_configs == 1 {
//         return CurrentMode::CDCNCMDirect;
//     }
//
//     if num_configs <= 4 {
//         return CurrentMode::Initial;
//     }
//
//     if num_configs == 6 {
//         return CurrentMode::USBEthWithCDCNCM;
//     }
//
//     if num_configs != 5 {
//         return CurrentMode::Undertermined;
//     }
//
//     let fifth_config = dev.configurations().nth(5).unwrap();
//
//     let mut has_valeria = false;
//     let mut has_cdc_ncm = false;
//     let mut has_usbmux = false;
//
//     for intf in fifth_config.interface_alt_settings() {
//         if intf.class() == 255 && intf.subclass() == 42 && intf.protocol() == 255 {
//             has_valeria = true;
//         }
//
//         if intf.class() == 2 && intf.subclass() == 0xd {
//             has_cdc_ncm = true;
//         }
//
//         if intf.class() == 255 && intf.subclass() == 254 && intf.protocol() == 2 {
//             has_usbmux = true;
//         }
//     }
//
//     if has_valeria && has_usbmux {
//         return CurrentMode::Valeria;
//     }
//
//     if has_cdc_ncm && has_usbmux {
//         return CurrentMode::CDCNCM;
//     }
//
//     CurrentMode::Undertermined
// }
