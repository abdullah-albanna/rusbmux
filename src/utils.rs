use std::borrow::Cow;

use nusb::{DeviceInfo, Speed};

pub(crate) fn nusb_speed_to_number(speed: Speed) -> u64 {
    match speed {
        Speed::Low => 1_500_000,
        Speed::Full => 12_000_000,
        Speed::High => 480_000_000,
        Speed::Super => 5_000_000_000,
        Speed::SuperPlus => 10_000_000_000,
        unknown => panic!("unknown device speed: {unknown:?}"),
    }
}

pub(crate) fn get_serial_number(device: &DeviceInfo) -> Cow<'_, str> {
    let serial_num = device.serial_number().unwrap_or_default();

    if serial_num.len() == 24 {
        let mut new_serial_num = String::with_capacity(25);
        new_serial_num.push_str(&serial_num[..8]);
        new_serial_num.push('-');
        new_serial_num.push_str(&serial_num[8..]);

        Cow::Owned(new_serial_num)
    } else {
        Cow::Borrowed(serial_num)
    }
}
