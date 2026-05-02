use tracing::{debug, error, info};

#[cfg(target_family = "unix")]
type Listener = tokio::net::UnixListener;
#[cfg(target_family = "unix")]
const LISTENER_PATH: &str = "/var/run/usbmuxd";

#[cfg(target_os = "windows")]
type Listener = tokio::net::TcpListener;
#[cfg(target_os = "windows")]
const LISTENER_PATH: &str = "127.0.0.1:27015";

async fn get_listener() -> Listener {
    let listener = Listener::bind(LISTENER_PATH);

    #[cfg(target_os = "windows")]
    let listener = listener.await.unwrap();

    #[cfg(target_family = "unix")]
    let listener = listener.unwrap();

    listener
}

#[cfg(feature = "bin")]
pub async fn run() {
    use crate::{
        handler::create_lockdown_dir,
        watcher::{device_watcher, network_watcher},
    };

    #[cfg(target_family = "unix")]
    {
        let socket_path = std::path::Path::new(LISTENER_PATH);
        if socket_path.exists() {
            debug!("Socket file already exists, removing...");

            std::fs::remove_file(socket_path).unwrap();
        }
    }

    let listener = get_listener().await;
    create_lockdown_dir().await.unwrap();

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

    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        debug!("Setting the socket permissions to 666");
        std::fs::set_permissions(LISTENER_PATH, std::fs::Permissions::from_mode(0o666)).unwrap();
    }

    info!("Spawning the device watcher");
    tokio::spawn(device_watcher());

    info!("Spawning the network watcher");
    tokio::spawn(network_watcher());

    tokio::select! {
        _ = start_accepting(listener) => {}
        _ = tokio::signal::ctrl_c()  => {
            info!("Got a Ctrl+C, closing...");
            cleanup().await;
        }

        _ = wait_shutdown() => {
            cleanup().await;
        }

    };

    // wait for RST packets, just in case
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
}

pub async fn wait_shutdown() {
    #[cfg(target_family = "unix")]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Got a SIGTERM signal, closing...");
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let mut shutdown = tokio::signal::windows::ctrl_shutdown().unwrap();
        let mut cbreak = tokio::signal::windows::ctrl_break().unwrap();
        let mut close = tokio::signal::windows::ctrl_close().unwrap();
        let mut logoff = tokio::signal::windows::ctrl_logoff().unwrap();
        tokio::select! {
            _ = shutdown.recv() => {
                info!("Got a shutdown signal, closing...");

            }
            _ = cbreak.recv() => {
                info!("Got a break signal, closing...");
            }

            _ = close.recv() => {
                info!("Got a close signal, closing...");
            }
            _ = logoff.recv() => {
                info!("Got a logoff signal, closing...");
            }
        }
    }
}

pub async fn cleanup() {
    for device in &*crate::watcher::CONNECTED_DEVICES {
        if let Err(e) = device.shutdown().await {
            error!(id = device.id(), ?e, "Failed to shutdown device");
        }
    }
}

#[cfg(feature = "bin")]
pub async fn start_accepting(listener: Listener) {
    use crate::handler;

    loop {
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
