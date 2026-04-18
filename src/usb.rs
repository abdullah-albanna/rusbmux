use std::pin::Pin;

use nusb::{
    descriptors::InterfaceDescriptor,
    io::{EndpointRead, EndpointWrite},
    transfer::{Bulk, Direction},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info, warn};

use crate::{error::RusbmuxError, parser::device_mux::UsbDevicePacket};

pub const APPLE_VID: u16 = 0x5ac;

pub async fn get_apple_device() -> impl Iterator<Item = nusb::DeviceInfo> {
    nusb::list_devices().await.unwrap().filter(|dev| {
        let is_apple = dev.vendor_id() == APPLE_VID;

        if is_apple {
            debug!(
                vid = dev.vendor_id(),
                pid = dev.product_id(),
                "Found an Apple device"
            );
        }

        is_apple
    })
}

pub const APPLE_USBMUX_CLASS: u8 = 255;
pub const APPLE_USBMUX_SUBCLASS: u8 = 254;
pub const APPLE_USBMUX_PROTOCOL: u8 = 2;

pub async fn get_usbmux_interface(
    dev: &nusb::Device,
) -> Result<InterfaceDescriptor<'_>, RusbmuxError> {
    let current_cfg = dev
        .active_configuration()
        .map_or(0, |c| c.configuration_value());

    debug!("Current device configuration: {current_cfg}");

    let (intf, intf_cfg_num) = dev
        .configurations()
        // search from the bottom up
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|cfg| (cfg.configuration_value(), cfg.interface_alt_settings()))
        .find_map(|(cfg_num, intfs)| {
            for intf in intfs {
                if intf.class() == APPLE_USBMUX_CLASS
                    && intf.subclass() == APPLE_USBMUX_SUBCLASS
                    && intf.protocol() == APPLE_USBMUX_PROTOCOL
                {
                    debug!(
                        configuration = cfg_num,
                        interface_number = intf.interface_number(),
                        "Found usbmux interface"
                    );

                    return Some((intf, cfg_num));
                }
            }
            None
        })
        .ok_or(RusbmuxError::UsbmuxInterfaceNotFound)?;

    // zero means it doesn't have any active configuration
    if intf_cfg_num != current_cfg || current_cfg == 0 {
        info!(
            old_cfg = current_cfg,
            new_cfg = intf_cfg_num,
            "Switching device configuration"
        );

        // TODO: maybe don't search for it again
        let cfg = dev
            .configurations()
            .find(|c| c.configuration_value() == intf_cfg_num)
            .unwrap();

        // make sure to detach any interfaces before setting the new configuration
        for intf in cfg.interface_alt_settings() {
            if intf.alternate_setting() != 0 {
                continue;
            }

            if let Err(e) = dev.detach_kernel_driver(intf.interface_number()) {
                warn!(
                    interface = intf.interface_number(),
                    error = ?e,
                    "Failed to detach kernel driver"
                );
            } else {
                debug!(
                    interface = intf.interface_number(),
                    "Detached kernel driver"
                );
            }
        }

        dev.set_configuration(intf_cfg_num).await?;
    }

    Ok(intf)
}

pub const MAX_PACKET_SIZE: usize = 32 * 1024;
pub const MAX_PACKET_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - UsbDevicePacket::HEADERS_LEN_V2;

pub async fn get_usb_endpoints<'a>(
    dev: &'a nusb::Device,
    interface_descriptor: &InterfaceDescriptor<'a>,
) -> Result<(EndpointRead<Bulk>, EndpointWrite<Bulk>), RusbmuxError> {
    let intf = dev
        .claim_interface(interface_descriptor.interface_number())
        .await?;

    let end_out = interface_descriptor
        .endpoints()
        .find(|ep| matches!(ep.direction(), Direction::Out))
        .ok_or(RusbmuxError::BulkOutEndpointNotFound)?
        .address();

    let end_in = interface_descriptor
        .endpoints()
        .find(|ep| matches!(ep.direction(), Direction::In))
        .ok_or(RusbmuxError::BulkInEndpointNotFound)?
        .address();

    debug!(
        interface = interface_descriptor.interface_number(),
        end_in, end_out, "Claimed interface and endpoints"
    );

    Ok((
        intf.endpoint(end_in)?.reader(MAX_PACKET_SIZE * 2),
        intf.endpoint(end_out)?.writer(MAX_PACKET_SIZE),
    ))
}

pub struct UsbStream {
    pub end_in: EndpointRead<Bulk>,
    pub end_out: EndpointWrite<Bulk>,
}

impl UsbStream {
    #[must_use]
    pub const fn new(end_in: EndpointRead<Bulk>, end_out: EndpointWrite<Bulk>) -> Self {
        Self { end_in, end_out }
    }
}

impl AsyncWrite for UsbStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.end_out).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.end_out).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.end_out).poll_shutdown(cx)
    }
}

impl AsyncRead for UsbStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.end_in).poll_read(cx, buf)
    }
}

impl std::fmt::Debug for UsbStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("end_in", &"...")
            .field("end_out", &"...")
            .finish()
    }
}
