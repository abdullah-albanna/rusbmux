#[cfg(target_os = "windows")]
compile_error!("windows is currently not supported due to how usb access is restricted");

#[cfg(feature = "bin")]
use std::os::unix::fs::PermissionsExt;

use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(feature = "bin")]
use device::device_watcher;

pub mod device;
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

#[cfg(feature = "bin")]
pub async fn run_daemon() {
    let socket_path = std::path::Path::new("/var/run/usbmuxd");

    if socket_path.exists() {
        std::fs::remove_file(socket_path).unwrap();
    }

    let listener = tokio::net::UnixListener::bind(socket_path).unwrap();

    #[cfg(target_family = "unix")]
    {
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
    }

    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666)).unwrap();

    tokio::spawn(device_watcher());

    loop {
        match listener.accept().await {
            Ok((mut socket, _addr)) => {
                tokio::spawn(async move {
                    handler::handle_client(&mut socket).await;
                });
            }
            Err(e) => eprintln!("couldn't accept the unix connection, error: {e:?}"),
        }
    }
}
