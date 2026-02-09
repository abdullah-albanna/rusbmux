use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use nix::sys::socket::{setsockopt, sockopt};
use rusbmux::parser::usbmux::UsbMuxPacket;
use tokio::net::UnixListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = Path::new("/var/run/usbmuxd");

    if socket_path.exists() {
        fs::remove_file(socket_path).unwrap();
    }

    let listener = UnixListener::bind(socket_path).unwrap();

    setsockopt(&listener, sockopt::ReuseAddr, &true)
        .expect("unable to set the `ReuseAddr` socket option");

    #[cfg(target_os = "macos")]
    setsockopt(&listener, sockopt::Nosigpipe, &true)
        .expect("unable to set the `Nosigpipe` socket option");

    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o666)).unwrap();

    loop {
        match listener.accept().await {
            Ok((mut socket, _addr)) => {
                tokio::spawn(async move {
                    let usbmux_packet = UsbMuxPacket::parse(&mut socket).await.unwrap();

                    println!("{usbmux_packet:#?}");
                    rusbmux::handler::handle_usbmux(usbmux_packet, &mut socket).await;
                });
            }
            Err(e) => eprintln!("accept function failed: {e:?}"),
        }
    }
}
