use std::os::unix::fs::PermissionsExt;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::handler::device_watcher::device_watcher;

pub mod handler;
pub mod parser;
pub mod usb;
pub mod utils;

pub trait ReadWrite: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> ReadWrite for T {}

pub trait AsyncReading: AsyncRead + Unpin + Send + Sync {}
impl<T: AsyncRead + Unpin + Send + Sync> AsyncReading for T {}

pub trait AsyncWriting: AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncWrite + Unpin + Send + Sync> AsyncWriting for T {}

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Attached {
        serial_number: String,
        id: u32,
        speed: u32,
        product_id: u16,
        device_address: u8,
    },
    Detached {
        id: u32,
    },
}

pub async fn run_daemon() {
    let socket_path = std::path::Path::new("/var/run/usbmuxd");

    if socket_path.exists() {
        std::fs::remove_file(socket_path).unwrap();
    }

    let listener = tokio::net::UnixListener::bind(socket_path).unwrap();

    nix::sys::socket::setsockopt(&listener, nix::sys::socket::sockopt::ReuseAddr, &true)
        .expect("unable to set the `ReuseAddr` socket option");

    // macos shuts the entire process if there's something wrong when reading or writing to the
    // socket, so this stops it
    #[cfg(target_os = "macos")]
    nix::sys::socket::sockopt::setsockopt(
        &listener,
        nix::sys::socket::sockopt::sockopt::Nosigpipe,
        &true,
    )
    .expect("unable to set the `Nosigpipe` socket option");

    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666)).unwrap();

    let (event_tx, _) = tokio::sync::broadcast::channel::<DeviceEvent>(32);

    tokio::spawn(device_watcher(event_tx.clone()));

    loop {
        match listener.accept().await {
            Ok((mut socket, _addr)) => {
                let event_tx_subscriber = event_tx.subscribe();
                tokio::spawn(async move {
                    handler::handle_client(&mut socket, event_tx_subscriber).await;
                });
            }
            Err(e) => eprintln!("accept function failed: {e:?}"),
        }
    }
}
