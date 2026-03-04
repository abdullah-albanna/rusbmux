use std::pin::Pin;

use nusb::{
    descriptors::InterfaceDescriptor,
    io::{EndpointRead, EndpointWrite},
    transfer::{Bulk, Direction},
};
use tokio::io::{AsyncRead, AsyncWrite};

pub const APPLE_VID: u16 = 0x5ac;

pub async fn get_apple_device() -> impl Iterator<Item = nusb::DeviceInfo> {
    nusb::list_devices()
        .await
        .unwrap()
        .filter(|dev| dev.vendor_id() == APPLE_VID)
}

pub const APPLE_USBMUX_CLASS: u8 = 255;
pub const APPLE_USBMUX_SUBCLASS: u8 = 254;
pub const APPLE_USBMUX_PROTOCOL: u8 = 2;

pub async fn get_usbmux_interface(dev: &nusb::Device) -> InterfaceDescriptor<'_> {
    let current_cfg = dev
        .active_configuration()
        .map_or(0, |c| c.configuration_value());

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
                    return Some((intf, cfg_num));
                }
            }
            None
        })
        .expect("there was no usbmux interface available");

    // zero means it doesn't have any active configuration
    if intf_cfg_num != current_cfg || current_cfg == 0 {
        println!("found the interface in another config");
        println!("the old config: {current_cfg}, new config: {intf_cfg_num}",);

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

            dev.detach_kernel_driver(intf.interface_number()).ok();
        }

        dev.set_configuration(intf_cfg_num).await.unwrap();
    }

    intf
}

pub async fn get_usb_endpoints<'a>(
    dev: &'a nusb::Device,
    interface_descriptor: &InterfaceDescriptor<'a>,
) -> (EndpointRead<Bulk>, EndpointWrite<Bulk>) {
    let intf = dev
        .claim_interface(interface_descriptor.interface_number())
        .await
        .expect("unable to claim the interface");

    let end_out = interface_descriptor
        .endpoints()
        .find(|ep| matches!(ep.direction(), Direction::Out))
        .expect("there was no bulk out endpoint")
        .address();

    let end_in = interface_descriptor
        .endpoints()
        .find(|ep| matches!(ep.direction(), Direction::In))
        .expect("there was no bulk in endpoint")
        .address();

    (
        intf.endpoint(end_in)
            .expect("failed to get bulk in")
            .reader(512),
        intf.endpoint(end_out)
            .expect("failed to get bulk in")
            .writer(512),
    )
}

pub struct UsbStream {
    pub end_in: EndpointRead<Bulk>,
    pub end_out: EndpointWrite<Bulk>,
}

impl UsbStream {
    pub fn new(end_in: EndpointRead<Bulk>, end_out: EndpointWrite<Bulk>) -> Self {
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
