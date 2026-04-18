use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{ResultCode, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
    watcher::{CONNECTED_DEVICES, DeviceEvent, HOTPLUG_EVENT_TX},
};

use tokio::{io::AsyncWriteExt, sync::broadcast};
use tracing::{debug, error, info, trace, warn};

pub async fn handle_listen(writer: &mut impl AsyncWriting, tag: u32) -> Result<(), RusbmuxError> {
    let mut event_receiver = match HOTPLUG_EVENT_TX
        .get()
        .ok_or(RusbmuxError::HotPlug)
        .map(broadcast::Sender::subscribe)
    {
        Ok(r) => r,
        Err(e) => {
            send_result(writer, ResultCode::BadDeviceOrNoSuchFile, tag).await?;
            return Err(e);
        }
    };

    send_result(writer, ResultCode::OK, tag).await?;

    send_currently_connected(writer, tag).await?;

    debug!(tag, "Listening for device attach/detach events");

    while let Ok(event) = event_receiver.recv().await {
        match event {
            DeviceEvent::Attached { id } => {
                info!(id, "Device attached");
                let device_plist = CONNECTED_DEVICES
                    .read()
                    .await
                    .iter()
                    .find(|dev| dev.id() == id)
                    .unwrap()
                    .create_device_attached()?;

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let connected_packet = UsbMuxPacket::encode_from(
                    device_xml,
                    UsbMuxVersion::Plist,
                    UsbMuxMsgType::MessagePlist,
                    tag,
                );
                writer.write_all(&connected_packet).await.inspect_err(
                    |e| error!(device_id = id, tag, err = ?e, "Failed to send device attach event"),
                )?;
                writer.flush().await.inspect_err(|e| error!(device_id = id, tag, err = ?e, "Failed to flush device attach event"))?;

                trace!(device_id = id, tag, "Attach event sent");
            }
            DeviceEvent::Detached { id } => {
                info!(id, "Device detached");

                let device_plist = plist_macro::plist!({
                    "MessageType": "Detached",
                    "DeviceID": id
                });

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let disconnected_packet = UsbMuxPacket::encode_from(
                    device_xml,
                    UsbMuxVersion::Plist,
                    UsbMuxMsgType::MessagePlist,
                    tag,
                );
                writer.write_all(&disconnected_packet).await.inspect_err(
                    |e| error!(device_id = id, tag, err = ?e, "Failed to send device detach event"),
                )?;
                writer.flush().await.inspect_err(|e| error!(device_id = id, tag, err = ?e, "Failed to flush device detach event"))?;

                trace!(device_id = id, tag, "Detach event sent");
            }
        }
    }

    warn!(tag, "Device listen session ended");

    Ok(())
}

pub async fn send_currently_connected(
    writer: &mut impl AsyncWriting,
    tag: u32,
) -> Result<(), RusbmuxError> {
    for device in CONNECTED_DEVICES.read().await.iter() {
        let device_plist = device.create_device_attached()?;

        let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

        let connected_packet = UsbMuxPacket::encode_from(
            device_xml,
            UsbMuxVersion::Plist,
            UsbMuxMsgType::MessagePlist,
            tag,
        );
        writer.write_all(&connected_packet).await.inspect_err(|e| error!(device_id = device.id(), tag, err = ?e, "Failed to send initial device packet"))?;
        writer.flush().await.inspect_err(|e| error!(device_id = device.id(), tag, err = ?e, "Failed to flush initial device packet"))?;
    }

    Ok(())
}
