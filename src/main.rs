use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use nix::sys::socket::{setsockopt, sockopt};
use rusbmux::{DeviceEvent, device_watcher};
use tokio::{net::UnixListener, sync::broadcast};

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

    let (event_tx, _) = broadcast::channel::<DeviceEvent>(32);

    tokio::spawn(device_watcher(event_tx.clone()));

    loop {
        match listener.accept().await {
            Ok((mut socket, _addr)) => {
                let event_tx_subscriber = event_tx.subscribe();
                tokio::spawn(async move {
                    rusbmux::handler::handle_client(&mut socket, event_tx_subscriber).await;
                });
            }
            Err(e) => eprintln!("accept function failed: {e:?}"),
        }
    }
}
