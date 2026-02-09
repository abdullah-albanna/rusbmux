use nusb::Speed;

pub(crate) fn nusb_speed_to_number(speed: Speed) -> u32 {
    match speed {
        Speed::Low => 1,
        Speed::Full => 12,
        Speed::High => 480,
        Speed::Super => 5000,
        Speed::SuperPlus => 10000,
        unknown => panic!("unknown device speed: {unknown:?}"),
    }
}
