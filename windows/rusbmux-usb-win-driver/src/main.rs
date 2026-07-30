use clap::Parser;
use wdi_rs::{CreateListOptions, InstallDriverOptions};
mod uninstall;

pub const APPLE_PID_RANGE: std::ops::RangeInclusive<u16> = 0x1290..=0x12af;
pub const APPLE_VID: u16 = 0x5ac;
pub const INF_DATA: &str = include_str!("../rusbmux_libusb0.inf");

#[derive(Parser, Debug)]
struct Args {
    /// force driver installation
    #[arg(long)]
    clean: bool,

    /// wait for user interaction / debug mode
    #[arg(long)]
    wait: bool,

    /// installs the usb driver
    #[arg(long)]
    install: bool,

    /// number of rescan cycles after uninstall (scan + uninstall repeated)
    /// used when multiple Windows drivers are stacked for the same device
    #[arg(long, default_value = "5")]
    rescans: u8,
}

fn press_to_continue() {
    use std::io::Read;
    println!("Press Enter to continue...");
    let mut buffer = [0u8; 1];
    let _ = std::io::stdin().read(&mut buffer);
}

fn main() {
    env_logger::init();
    wdi_rs::set_log_level(log::Level::Trace.into()).unwrap();

    let args = Args::parse();

    if args.clean
        && let Err(code) = uninstall::uninstall_any_apple_driver(args.rescans)
    {
        std::process::exit(code);
    }

    if !args.install {
        std::process::exit(0);
    }

    let devices = match wdi_rs::create_list(CreateListOptions {
        list_all: true,
        list_hubs: true,
        trim_whitespaces: true,
    }) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Couldn't list devices: {e}");
            if args.wait {
                press_to_continue();
            }
            std::process::exit(-667);
        }
    };

    let Some(device) = devices.iter().find(|dev| {
        dev.vid == APPLE_VID && APPLE_PID_RANGE.contains(&dev.pid) && !dev.is_composite
    }) else {
        eprintln!("No Apple device is plugged in");
        if args.wait {
            press_to_continue();
        }
        std::process::exit(-677);
    };

    if let Err(e) = wdi_rs::DriverInstaller::new(wdi_rs::DeviceSelector::Specific(device))
        .with_driver_type(wdi_rs::DriverType::LibUsb0)
        .with_install_options(InstallDriverOptions {
            install_filter_driver: false,
            pending_install_timeout: InstallDriverOptions::DEFAULT_PENDING_INSTALL_TIMEOUT * 4,
        })
        // libwdi doesn't like CRLF
        .with_inf_data(INF_DATA.replace('\r', "").as_bytes(), "rusbmux_libusb0.inf")
        .with_force(true)
        .install()
    {
        eprintln!("Couldn't install libusb-win32 driver: {e}");
        if args.wait {
            press_to_continue();
        }
        std::process::exit(-67);
    };

    if args.wait {
        press_to_continue();
    }
}
