use std::{
    borrow::Cow,
    fmt::{self, Debug},
    pin::Pin,
    sync::{Arc, atomic::AtomicU64},
};

use futures_lite::Stream;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{AsyncReading, AsyncWriting, error::RusbmuxError, parser::device_mux::UsbDevicePacket};
mod nusb;
mod rusb;

#[cfg(target_os = "windows")]
pub const DEFAULT_BACKEND: rusb::RusbBackend = rusb::RusbBackend;

#[cfg(not(target_os = "windows"))]
pub const DEFAULT_BACKEND: nusb::NusbBackend = nusb::NusbBackend;

pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

pub const APPLE_VID: u16 = 0x5ac;

pub const APPLE_USBMUX_CLASS: u8 = 255;
pub const APPLE_USBMUX_SUBCLASS: u8 = 254;
pub const APPLE_USBMUX_PROTOCOL: u8 = 2;

pub const MAX_PACKET_SIZE: usize = 48 * 1024;
pub const MAX_PACKET_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - UsbDevicePacket::HEADERS_LEN_V2;

// descripes how to flush with ZLP end
pub trait UsbAsyncWriteEndpoint: AsyncWriting {
    fn submit_end(&mut self);
}

pub struct Endpoint {
    pub reader: Box<dyn AsyncReading>,
    pub writer: Box<dyn UsbAsyncWriteEndpoint>,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum AnyDeviceInfo {
    Nusb(::nusb::DeviceInfo),
    Rusb(::rusb::Device<::rusb::GlobalContext>),
}

#[derive(Debug)]
#[non_exhaustive]
pub enum AnyDeviceHandle {
    Nusb(::nusb::Device),
    Rusb {
        handle: Arc<::rusb::DeviceHandle<::rusb::GlobalContext>>,
        end_in: u8,
        end_out: u8,
        max_packet_size: u16,
    },
}

#[non_exhaustive]
pub enum AnyEndpointReader {
    Nusb(::nusb::io::EndpointRead<::nusb::transfer::Bulk>),
    Rusb(rusb::RusbAsyncReader),
}

#[non_exhaustive]
pub enum AnyEndpointWriter {
    Nusb(::nusb::io::EndpointWrite<::nusb::transfer::Bulk>),
    Rusb(rusb::RusbAsyncWriter),
}

impl fmt::Debug for AnyEndpointReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nusb(_) => f.debug_tuple("NusbReader").field(&"...").finish(),
            Self::Rusb(_) => f.debug_tuple("RusbReader").field(&"...").finish(),
        }
    }
}

impl fmt::Debug for AnyEndpointWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nusb(_) => f.debug_tuple("NusbWriter").field(&"...").finish(),
            Self::Rusb(_) => f.debug_tuple("RusbWriter").field(&"...").finish(),
        }
    }
}

impl AsyncRead for AnyEndpointReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Nusb(r) => Pin::new(r).poll_read(cx, buf),
            Self::Rusb(r) => Pin::new(r).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for AnyEndpointWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Nusb(w) => Pin::new(w).poll_write(cx, buf),
            Self::Rusb(w) => Pin::new(w).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Nusb(w) => Pin::new(w).poll_flush(cx),
            Self::Rusb(w) => Pin::new(w).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Nusb(w) => Pin::new(w).poll_shutdown(cx),
            Self::Rusb(w) => Pin::new(w).poll_shutdown(cx),
        }
    }
}

impl UsbAsyncWriteEndpoint for AnyEndpointWriter {
    fn submit_end(&mut self) {
        match self {
            Self::Nusb(w) => w.submit_end(),
            Self::Rusb(w) => {
                let _ = w.submit_end();
            }
        }
    }
}

impl AnyDeviceInfo {
    pub fn vendor_id(&self) -> u16 {
        match self {
            Self::Nusb(info) => info.vendor_id(),
            Self::Rusb(dev) => dev.device_descriptor().expect("shouldn't fail").vendor_id(),
        }
    }

    pub fn product_id(&self) -> u16 {
        match self {
            Self::Nusb(info) => info.product_id(),
            Self::Rusb(dev) => dev
                .device_descriptor()
                .expect("shouldn't fail")
                .product_id(),
        }
    }

    pub fn serial_number(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Nusb(info) => info.serial_number().map(crate::utils::get_serial_number),
            Self::Rusb(dev) => {
                let desc = dev.device_descriptor().expect("shouldn't fail");
                let serial_number = rusb::get_serial_number(dev, &desc);

                serial_number
                    .map(crate::utils::get_serial_number_owned)
                    .map(Cow::Owned)
            }
        }
    }

    pub fn speed(&self) -> Option<u64> {
        match self {
            Self::Nusb(info) => info.speed().map(crate::utils::nusb_speed_to_number),
            Self::Rusb(dev) => {
                let speed = dev.speed();

                if matches!(speed, ::rusb::Speed::Unknown) {
                    return None;
                }

                Some(crate::utils::rusb_speed_to_number(speed))
            }
        }
    }

    pub fn location_id(&self) -> u32 {
        match self {
            Self::Nusb(info) => {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                {
                    (info.busnum() as u32) << 16 | info.device_address() as u32
                }

                #[cfg(target_os = "macos")]
                {
                    info.location_id()
                }

                #[cfg(target_os = "windows")]
                {
                    0
                }
            }

            Self::Rusb(dev) => {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                {
                    (dev.bus_number() as u32) << 16 | dev.address() as u32
                }

                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    0
                }
            }
        }
    }

    pub fn bus_number(&self) -> u8 {
        match self {
            Self::Nusb(info) => {
                #[cfg(not(target_os = "windows"))]
                {
                    info.busnum()
                }

                #[cfg(target_os = "windows")]
                {
                    0
                }
            }
            Self::Rusb(dev) => dev.bus_number(),
        }
    }

    pub fn device_address(&self) -> u8 {
        match self {
            Self::Nusb(info) => info.device_address(),
            Self::Rusb(dev) => dev.address(),
        }
    }

    pub fn opaque_id(&self) -> u64 {
        match self {
            Self::Nusb(info) => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::hash::DefaultHasher::new();
                info.id().hash(&mut hasher);
                hasher.finish()
            }
            Self::Rusb(dev) => rusb::opaque_id(dev),
        }
    }

    pub async fn open(&self) -> Result<AnyDeviceHandle, RusbmuxError> {
        match self {
            Self::Nusb(info) => Ok(AnyDeviceHandle::Nusb(::nusb::DeviceInfo::open(info).await?)),
            Self::Rusb(dev) => {
                let dev_handle = Arc::new(dev.open()?);

                let (end_in, end_out, max_packet_size) =
                    rusb::device_endpoints(dev, Arc::clone(&dev_handle))?;

                Ok(AnyDeviceHandle::Rusb {
                    handle: dev_handle,
                    end_in,
                    end_out,
                    max_packet_size,
                })
            }
        }
    }
}

impl AnyDeviceHandle {
    pub async fn endpoint(&self) -> Result<(AnyEndpointReader, AnyEndpointWriter), RusbmuxError> {
        match self {
            Self::Nusb(dev) => {
                let (reader, writer) = nusb::device_endpoints(dev).await?;
                Ok((
                    AnyEndpointReader::Nusb(reader),
                    AnyEndpointWriter::Nusb(writer),
                ))
            }
            Self::Rusb {
                handle,
                end_in,
                end_out,
                max_packet_size,
            } => {
                let reader = rusb::RusbAsyncReader::new(Arc::clone(handle), *end_in);
                let writer =
                    rusb::RusbAsyncWriter::new(Arc::clone(handle), *end_out, *max_packet_size);

                Ok((
                    AnyEndpointReader::Rusb(reader),
                    AnyEndpointWriter::Rusb(writer),
                ))
            }
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Connected(AnyDeviceInfo, u64),
    Disconnected(u64),
}

pub trait UsbBackend: Send + Sync + 'static {
    fn list_devices(&self) -> impl std::future::Future<Output = Vec<AnyDeviceInfo>> + Send;
    fn watch_devices(
        &self,
    ) -> impl std::future::Future<
        Output = Result<BoxStream<Result<Event, RusbmuxError>>, RusbmuxError>,
    > + Send;
}

pub static DEVICE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[inline]
pub fn take_new_id() -> u64 {
    DEVICE_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
