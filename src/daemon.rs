#[cfg(feature = "bin")]
pub async fn run() {
    use crate::watcher::device_watcher;
    use std::os::unix::fs::PermissionsExt;

    let socket_path = std::path::Path::new("/var/run/usbmuxd");

    if socket_path.exists() {
        std::fs::remove_file(socket_path).unwrap();
    }

    let listener = tokio::net::UnixListener::bind(socket_path).unwrap();

    #[cfg(target_family = "unix")]
    {
        rustix::net::sockopt::set_socket_reuseaddr(&listener, true)
            .expect("unable to set the `ReuseAddr` socket option");

        // macos shuts the entire process if there's something wrong when reading or writing to the
        // socket, so this stops it
        #[cfg(target_os = "macos")]
        rustix::net::sockopt::set_socket_nosigpipe(&listener, true)
            .expect("unable to set the `Nosigpipe` socket option");
    }

    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666)).unwrap();

    tokio::spawn(device_watcher());

    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();

    tokio::select! {
        _ = start_accepting(listener) => {}
        _ = tokio::signal::ctrl_c()  => {
            cleanup().await;
        }
        _ =  sigterm.recv() => {
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
        match listener.accept().await {
            Ok((socket, _addr)) => {
                tokio::spawn(async move {
                    handler::handle_client(Box::new(socket)).await;
                });
            }
            Err(e) => eprintln!("couldn't accept the unix connection, error: {e:?}"),
        }
    }
}
