use nusb::Speed;

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
