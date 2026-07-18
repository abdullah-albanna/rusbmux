use crate::error::RusbmuxError;
use tracing::{debug, error, info, warn};

#[cfg(unix)]
type Listener = tokio::net::UnixListener;
#[cfg(unix)]
const LISTENER_PATH: &str = "/var/run/usbmuxd";

#[cfg(windows)]
type Listener = tokio::net::TcpListener;
#[cfg(windows)]
const LISTENER_PATH: &str = "127.0.0.1:27015";

async fn get_listener() -> Result<Listener, RusbmuxError> {
    let listener = Listener::bind(LISTENER_PATH);

    #[cfg(windows)]
    let listener = listener.await?;

    #[cfg(unix)]
    let listener = listener?;

    Ok(listener)
}

#[cfg(feature = "bin")]
pub async fn run() -> Result<(), RusbmuxError> {
    use crate::{
        handler::create_lockdown_dir,
        watcher::{watch_network_daemon, watch_usb_daemon},
    };

    #[cfg(unix)]
    {
        let socket_path = std::path::Path::new(LISTENER_PATH);
        if socket_path.exists() {
            debug!("Socket file already exists, removing...");

            if let Err(e) = std::fs::remove_file(socket_path) {
                warn!(err = ?e, "Failed to remove existing socket file");
            }
        }
    }

    let listener = get_listener().await?;
    if let Err(e) = create_lockdown_dir().await {
        error!(err = ?e, "Failed to create lockdown directory");
    }

    #[cfg(unix)]
    {
        debug!("Setting the `ReuseAddr` socket option");
        if let Err(e) = rustix::net::sockopt::set_socket_reuseaddr(&listener, true) {
            warn!(err = ?e, "Failed to set ReuseAddr socket option");
        }

        // macos shuts the entire process if there's something wrong when reading or writing to the
        // socket, so this stops it
        #[cfg(target_os = "macos")]
        {
            debug!("Setting the `Nosigpipe` socket option");
            if let Err(e) = rustix::net::sockopt::set_socket_nosigpipe(&listener, true) {
                warn!(err = ?e, "Failed to set Nosigpipe socket option");
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        debug!("Setting the socket permissions to 666");
        if let Err(e) =
            std::fs::set_permissions(LISTENER_PATH, std::fs::Permissions::from_mode(0o666))
        {
            warn!(err = ?e, "Failed to set socket permissions");
        }
    }

    info!("Spawning the device watcher");
    tokio::spawn(watch_usb_daemon(crate::usb_backend::DEFAULT_BACKEND));

    info!("Spawning the network watcher");
    tokio::spawn(watch_network_daemon());

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

    Ok(())
}

#[cfg(feature = "bin")]
pub async fn wait_shutdown() {
    #[cfg(unix)]
    {
        let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            warn!("Failed to register SIGTERM handler");
            return;
        };

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Got a SIGTERM signal, closing...");
            }
        }
    }

    #[cfg(windows)]
    {
        let mut shutdown = match tokio::signal::windows::ctrl_shutdown() {
            Ok(s) => s,
            Err(e) => {
                warn!(err = ?e, "Failed to register ctrl_shutdown handler");
                return;
            }
        };
        let mut cbreak = match tokio::signal::windows::ctrl_break() {
            Ok(s) => s,
            Err(e) => {
                warn!(err = ?e, "Failed to register ctrl_break handler");
                return;
            }
        };
        let mut close = match tokio::signal::windows::ctrl_close() {
            Ok(s) => s,
            Err(e) => {
                warn!(err = ?e, "Failed to register ctrl_close handler");
                return;
            }
        };
        let mut logoff = match tokio::signal::windows::ctrl_logoff() {
            Ok(s) => s,
            Err(e) => {
                warn!(err = ?e, "Failed to register ctrl_logoff handler");
                return;
            }
        };
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
