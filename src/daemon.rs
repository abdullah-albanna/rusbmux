use tracing::{debug, info};

#[cfg(feature = "bin")]
pub async fn run() {
    use crate::watcher::device_watcher;
    use std::os::unix::fs::PermissionsExt;

    let socket_path = std::path::Path::new("/var/run/usbmuxd");

    if socket_path.exists() {
        debug!("Socket file already exists, removing...");

        std::fs::remove_file(socket_path).unwrap();
    }

    let listener = tokio::net::UnixListener::bind(socket_path).unwrap();

    #[cfg(target_family = "unix")]
    {
        debug!("Setting the `ReuseAddr` socket option");
        rustix::net::sockopt::set_socket_reuseaddr(&listener, true)
            .expect("unable to set the `ReuseAddr` socket option");

        // macos shuts the entire process if there's something wrong when reading or writing to the
        // socket, so this stops it
        #[cfg(target_os = "macos")]
        {
            debug!("Setting the `Nosigpipe` socket option");
            rustix::net::sockopt::set_socket_nosigpipe(&listener, true)
                .expect("unable to set the `Nosigpipe` socket option");
        }
    }

    debug!("Setting the socket permissions to 666");
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666)).unwrap();

    info!("Spawning the device watcher");
    tokio::spawn(device_watcher());

    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();

    tokio::select! {
        _ = start_accepting(listener) => {}
        _ = tokio::signal::ctrl_c()  => {
            info!("Got a Ctrl+C, closing...");
            cleanup().await;
        }
        _ =  sigterm.recv() => {
            info!("Got a termination signal, closing...");
            cleanup().await;
        }
    }
}

pub async fn cleanup() {
    for device in &*crate::watcher::CONNECTED_DEVICES.read().await {
        device.close_all().await;
    }
}

#[cfg(feature = "bin")]
pub async fn start_accepting(listener: tokio::net::UnixListener) {
    use crate::handler;

    loop {
        use tracing::error;

        match listener.accept().await {
            Ok((socket, _)) => {
                info!("New connection");
                tokio::spawn(async move {
                    handler::handle_client(Box::new(socket)).await;
                });
            }
            Err(e) => error!("Unable to accept the unix connection: {e:?}"),
        }
    }
}
