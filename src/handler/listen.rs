use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::{ResultCode, device_list::create_device_connected_plist, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
    utils::{self, nusb_speed_to_number},
    watcher::{CONNECTED_DEVICES, DeviceEvent, HOTPLUG_EVENT_TX},
};

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, trace, warn};

pub async fn handle_listen(writer: &mut impl AsyncWriting, tag: u32) -> Result<(), RusbmuxError> {
    let mut event_receiver = match HOTPLUG_EVENT_TX
        .get()
        .ok_or(RusbmuxError::HotPlug)
        .map(|tx| tx.subscribe())
    {
        Ok(r) => r,
        Err(e) => {
            // TODO: send result not okay
            send_result(writer, ResultCode::BadDeviceOrNoSuchFile, tag).await?;
            return Err(e);
        }
    };

    send_result(writer, ResultCode::OK, tag).await?;

    send_currently_connected(writer, tag).await?;

    debug!(tag, "Listening for device attach/detach events");

    while let Ok(event) = event_receiver.recv().await {
        match event {
            DeviceEvent::Attached {
                serial_number,
                id,
                speed,
                product_id,
                location_id,
            } => {
                debug!(
                    device_id = id,
                    serial_number, speed, location_id, product_id, "Device attached"
                );
                let device_plist = create_device_connected_plist(
                    id as _,
                    speed,
                    location_id,
                    product_id,
                    serial_number,
                );

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
                info!(device_id = id, "Device detached");

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
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let location_id = (device.info.busnum() as u32) << 16 | device.info.device_address() as u32;

        #[cfg(target_os = "macos")]
        let location_id = device.info.location_id();

        let speed = nusb_speed_to_number(device.info.speed().unwrap_or(nusb::Speed::Low));
        let serial_number = utils::get_serial_number(&device.info).to_string();

        debug!(
            device_id = device.id,
            serial_number,
            speed,
            location_id,
            product_id = device.info.product_id(),
            "Sending initial device info"
        );

        let device_plist = create_device_connected_plist(
            device.id,
            speed,
            location_id,
            device.info.product_id(),
            serial_number,
        );

        let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

        let connected_packet = UsbMuxPacket::encode_from(
            device_xml,
            UsbMuxVersion::Plist,
            UsbMuxMsgType::MessagePlist,
            tag,
        );
        writer.write_all(&connected_packet).await.inspect_err(|e| error!(device_id = device.id, tag, err = ?e, "Failed to send initial device packet"))?;
        writer.flush().await.inspect_err(|e| error!(device_id = device.id, tag, err = ?e, "Failed to flush initial device packet"))?;
    }

    Ok(())
}
