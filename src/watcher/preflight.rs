use std::{io::ErrorKind, path::Path, sync::Arc};

use idevice::{
    Idevice, IdeviceError,
    pairing_file::PairingFile,
    services::{lockdown::LockdownClient, notification_proxy::NotificationProxyClient},
};
use tracing::error;

use crate::{
    device::usb::UsbDevice,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, connect::handle_usb_device_connect, read_buid::read_system_buid},
};

const LOCKDOWN_PORT: u16 = 62078;
const STREAM_BUFFER_SIZE: usize = 128 * 1024;
const INSECURE_NOTIFICATION_PROXY: &str = "com.apple.mobile.insecure_notification_proxy";
const REQUEST_PAIR: &str = "com.apple.mobile.lockdown.request_pair";
const REQUEST_HOST_BUID: &str = "com.apple.mobile.lockdown.request_host_buid";

pub(super) fn spawn(device: Arc<UsbDevice>) {
    tokio::spawn(async move {
        if let Err(error) = run(device).await {
            error!(?error, "Device preflight failed");
        }
    });
}

async fn run(device: Arc<UsbDevice>) -> Result<(), RusbmuxError> {
    let path = Path::new(LOCKDOWN_PATH).join(format!("{}.plist", device.serial_number));

    let pairing_file = match tokio::fs::read(&path).await {
        Ok(data) => Some(PairingFile::from_bytes(&data)?),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    if let Some(pairing_file) = pairing_file {
        match connect(&device).await?.start_session(&pairing_file).await {
            Ok(_) => return Ok(()),
            Err(IdeviceError::InvalidHostID) => {}
            Err(error) => return Err(error.into()),
        }
    }

    let system_buid = read_system_buid().await?;
    let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let mut lockdown = connect(&device).await?;

    lockdown
        .set_value("UntrustedHostBUID", system_buid.as_str().into(), None)
        .await?;

    let pairing_file = match lockdown.pair_once(&host_id, &system_buid, None).await {
        Ok(pairing_file) => pairing_file,
        Err(IdeviceError::PasswordProtected | IdeviceError::PairingDialogResponsePending) => {
            let mut notifications = connect_notifications(&device, &mut lockdown).await?;
            wait_for_pair_request(&mut lockdown, &mut notifications, &system_buid).await?;
            lockdown.pair_once(host_id, system_buid, None).await?
        }
        Err(error) => return Err(error.into()),
    };
    tokio::fs::write(path, pairing_file.serialize()?).await?;

    Ok(())
}

async fn connect(device: &Arc<UsbDevice>) -> Result<LockdownClient, RusbmuxError> {
    Ok(LockdownClient::new(
        connect_idevice(device, LOCKDOWN_PORT).await?,
    ))
}

async fn connect_notifications(
    device: &Arc<UsbDevice>,
    lockdown: &mut LockdownClient,
) -> Result<NotificationProxyClient, RusbmuxError> {
    let (port, _) = lockdown.start_service(INSECURE_NOTIFICATION_PROXY).await?;
    let mut notifications = NotificationProxyClient::new(connect_idevice(device, port).await?);

    notifications
        .observe_notifications(&[REQUEST_PAIR, REQUEST_HOST_BUID])
        .await?;

    Ok(notifications)
}

async fn wait_for_pair_request(
    lockdown: &mut LockdownClient,
    notifications: &mut NotificationProxyClient,
    system_buid: &str,
) -> Result<(), RusbmuxError> {
    loop {
        match notifications.receive_notification().await?.as_str() {
            REQUEST_PAIR => return Ok(()),
            REQUEST_HOST_BUID => {
                lockdown
                    .set_value("UntrustedHostBUID", system_buid.into(), None)
                    .await?;
            }
            _ => {}
        }
    }
}

async fn connect_idevice(device: &Arc<UsbDevice>, port: u16) -> Result<Idevice, RusbmuxError> {
    let connection = device.connect(port).await?;
    let (stream, proxy) = tokio::io::duplex(STREAM_BUFFER_SIZE);

    tokio::spawn(async move {
        let _ = handle_usb_device_connect(Box::new(proxy), connection).await;
    });

    Ok(Idevice::new(Box::new(stream), "rusbmux"))
}
