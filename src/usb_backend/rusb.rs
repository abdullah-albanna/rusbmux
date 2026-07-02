use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    sync::{
        Arc, Once,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use rusb::UsbContext;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info};

use crate::{
    error::RusbmuxError,
    usb_backend::{MAX_PACKET_PAYLOAD_SIZE, MAX_PACKET_SIZE, take_new_id},
};

use super::{
    APPLE_USBMUX_CLASS, APPLE_USBMUX_PROTOCOL, APPLE_USBMUX_SUBCLASS, APPLE_VID, AnyDeviceInfo,
    UsbBackend,
};

fn io_error(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

static ENSURE_EVENTS: Once = Once::new();

fn ensure_event_thread() {
    ENSURE_EVENTS.call_once(|| {
        std::thread::Builder::new()
            .name("libusb-events".into())
            .spawn(move || {
                let ctx = rusb::GlobalContext::default();
                loop {
                    let _ = ctx.handle_events(None);
                }
            })
            .expect("failed to spawn libusb event thread");
    });
}

struct BulkResult {
    status: i32,
    data: Vec<u8>,
}

struct TransferUserData {
    result: tokio::sync::oneshot::Sender<BulkResult>,
    buffer_ptr: *mut u8,
    buffer_len: usize,
    completed: Arc<AtomicBool>,
}

unsafe impl Send for TransferUserData {}
unsafe impl Sync for TransferUserData {}

extern "system" fn bulk_callback(transfer: *mut rusb::ffi::libusb_transfer) {
    unsafe {
        let ud = Box::from_raw((*transfer).user_data as *mut TransferUserData);

        ud.completed.store(true, Ordering::SeqCst);

        let actual_length = (*transfer).actual_length.max(0) as usize;
        let status = (*transfer).status;

        let data = if actual_length > 0 {
            std::slice::from_raw_parts(ud.buffer_ptr, actual_length).to_vec()
        } else {
            Vec::new()
        };

        let _ = ud.result.send(BulkResult { status, data });

        // reconstruct the original Vec<u8> and drop to free the buffer
        let _ = Vec::from_raw_parts(ud.buffer_ptr, 0, ud.buffer_len);

        rusb::ffi::libusb_free_transfer(transfer);
    }
}

struct TransferHandle {
    transfer: *mut rusb::ffi::libusb_transfer,
    completed: Arc<AtomicBool>,
}

unsafe impl Send for TransferHandle {}
unsafe impl Sync for TransferHandle {}

impl Drop for TransferHandle {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::SeqCst) && !self.transfer.is_null() {
            unsafe {
                let _ = rusb::ffi::libusb_cancel_transfer(self.transfer);
            }
        }
    }
}

fn alloc_and_submit(
    handle: &rusb::DeviceHandle<rusb::GlobalContext>,
    endpoint: u8,
    buffer: Vec<u8>,
) -> Result<(TransferHandle, tokio::sync::oneshot::Receiver<BulkResult>), RusbmuxError> {
    ensure_event_thread();

    let length = buffer.len() as std::os::raw::c_int;
    let (tx, rx) = tokio::sync::oneshot::channel();

    let transfer: *mut rusb::ffi::libusb_transfer = unsafe { rusb::ffi::libusb_alloc_transfer(0) };
    if transfer.is_null() {
        return Err(RusbmuxError::UnexpectedPacket(
            "libusb_alloc_transfer failed".to_string(),
        ));
    }

    let buffer_ptr = buffer.as_ptr() as *mut u8;
    let buffer_cap = buffer.capacity();
    std::mem::forget(buffer);

    let completed = Arc::new(AtomicBool::new(false));

    let user_data = Box::into_raw(Box::new(TransferUserData {
        result: tx,
        buffer_ptr,
        buffer_len: buffer_cap,
        completed: Arc::clone(&completed),
    }));

    unsafe {
        rusb::ffi::libusb_fill_bulk_transfer(
            transfer,
            handle.as_raw(),
            endpoint,
            buffer_ptr,
            length,
            bulk_callback,
            user_data as *mut std::ffi::c_void,
            0,
        );

        let ret = rusb::ffi::libusb_submit_transfer(transfer);
        if ret != 0 {
            let _ = Box::from_raw(user_data);
            let _ = Vec::from_raw_parts(buffer_ptr, 0, buffer_cap);
            rusb::ffi::libusb_free_transfer(transfer);
            return Err(RusbmuxError::UnexpectedPacket(format!(
                "libusb_submit_transfer error: {ret}"
            )));
        }
    }

    Ok((
        TransferHandle {
            transfer,
            completed,
        },
        rx,
    ))
}

fn libusb_status_str(status: i32) -> String {
    match status {
        -1 => "LIBUSB_ERROR_IO".into(),
        -2 => "LIBUSB_ERROR_INVALID_PARAM".into(),
        -3 => "LIBUSB_ERROR_ACCESS".into(),
        -4 => "LIBUSB_ERROR_NO_DEVICE".into(),
        -5 => "LIBUSB_ERROR_NOT_FOUND".into(),
        -6 => "LIBUSB_ERROR_BUSY".into(),
        -7 => "LIBUSB_ERROR_TIMEOUT".into(),
        -8 => "LIBUSB_ERROR_OVERFLOW".into(),
        -9 => "LIBUSB_ERROR_PIPE".into(),
        -10 => "LIBUSB_ERROR_INTERRUPTED".into(),
        -11 => "LIBUSB_ERROR_NO_MEM".into(),
        -12 => "LIBUSB_ERROR_NOT_SUPPORTED".into(),
        _ => format!("LIBUSB_ERROR_UNKNOWN({status})"),
    }
}

#[allow(unused)]
pub struct RusbBackend;

struct PollingStream {
    known: HashMap<u64, rusb::Device<rusb::GlobalContext>>,
    pending: VecDeque<UsbEvent>,
}

impl PollingStream {
    fn new() -> Self {
        Self {
            known: HashMap::new(),
            pending: VecDeque::new(),
        }
    }

    async fn next(&mut self) -> Option<UsbEvent> {
        loop {
            {
                if let Some(event) = self.pending.pop_front() {
                    return Some(event);
                }

                let devices = if let Ok(devices) = rusb::devices() {
                    devices
                } else {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    continue;
                };

                let mut current = HashMap::new();

                for dev in devices.iter() {
                    let Ok(desc) = dev.device_descriptor() else {
                        continue;
                    };

                    if desc.vendor_id() != APPLE_VID {
                        continue;
                    }

                    current.insert(opaque_id(&dev), dev);
                }

                // arrivals
                for (&id, dev) in &current {
                    if !self.known.contains_key(&id) {
                        self.pending.push_back(UsbEvent::Arrived(dev.clone()));
                    }
                }

                // removals
                for (&id, dev) in &self.known {
                    if !current.contains_key(&id) {
                        self.pending.push_back(UsbEvent::Left(dev.clone()));
                    }
                }

                self.known = current;

                if let Some(event) = self.pending.pop_front() {
                    return Some(event);
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
}

enum DeviceStream {
    Hotplug(HotplugStream),
    Polling(PollingStream),
}

impl DeviceStream {
    async fn next(&mut self) -> Option<UsbEvent> {
        match self {
            DeviceStream::Hotplug(stream) => stream.next().await,
            DeviceStream::Polling(stream) => stream.next().await,
        }
    }
}

enum UsbEvent {
    Arrived(::rusb::Device<::rusb::GlobalContext>),
    Left(::rusb::Device<::rusb::GlobalContext>),
}

struct Callback {
    tx: tokio::sync::mpsc::UnboundedSender<UsbEvent>,
}

impl ::rusb::Hotplug<::rusb::GlobalContext> for Callback {
    fn device_arrived(&mut self, device: ::rusb::Device<::rusb::GlobalContext>) {
        let _ = self.tx.send(UsbEvent::Arrived(device));
    }

    fn device_left(&mut self, device: ::rusb::Device<::rusb::GlobalContext>) {
        let _ = self.tx.send(UsbEvent::Left(device));
    }
}

struct HotplugStream {
    _registration: ::rusb::Registration<::rusb::GlobalContext>,
    rx: tokio::sync::mpsc::UnboundedReceiver<UsbEvent>,
}

impl HotplugStream {
    fn new() -> rusb::Result<Self> {
        if !rusb::has_hotplug() {
            return Err(rusb::Error::NotSupported);
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let registration = ::rusb::HotplugBuilder::new()
            .enumerate(true)
            .vendor_id(APPLE_VID)
            .register(::rusb::GlobalContext::default(), Box::new(Callback { tx }))?;

        Ok(Self {
            _registration: registration,
            rx,
        })
    }

    async fn next(&mut self) -> Option<UsbEvent> {
        self.rx.recv().await
    }
}

impl UsbBackend for RusbBackend {
    async fn list_devices(&self) -> Vec<AnyDeviceInfo> {
        let devices = match rusb::devices() {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(err = ?e, "Failed to list USB devices");
                return Vec::new();
            }
        };

        devices
            .iter()
            .filter(|dev| {
                if let Ok(desc) = dev.device_descriptor() {
                    return desc.vendor_id() == APPLE_VID;
                }
                false
            })
            .map(AnyDeviceInfo::Rusb)
            .collect()
    }

    async fn watch_devices(
        &self,
    ) -> Result<
        super::BoxStream<Result<super::Event, crate::error::RusbmuxError>>,
        crate::error::RusbmuxError,
    > {
        ensure_event_thread();

        let mut stream = if rusb::has_hotplug() {
            DeviceStream::Hotplug(
                HotplugStream::new().map_err(|_| RusbmuxError::HotPlugNotSupported)?,
            )
        } else {
            info!("libusb hotplug unsupported, falling back to polling");
            DeviceStream::Polling(PollingStream::new())
        };

        Ok(Box::pin(async_stream::stream! {
            let mut devices_id_map = HashMap::new();

            while let Some(event) = stream.next().await {
                match event {
                    UsbEvent::Arrived(dev) => {
                        let id = take_new_id();

                        devices_id_map.insert(opaque_id(&dev), id);
                        yield Ok(super::Event::Connected(AnyDeviceInfo::Rusb(dev), id));
                    },
                    UsbEvent::Left(dev) => if let Some(id) = devices_id_map.get(&opaque_id(&dev)) {
                        yield Ok(super::Event::Disconnected(*id))
                    }
                }
            }
        }))
    }
}

pub fn get_serial_number(
    device: &rusb::Device<rusb::GlobalContext>,
    desc: &rusb::DeviceDescriptor,
) -> Option<String> {
    let timeout = Duration::from_secs(1);
    let handle = device.open().ok()?;
    let languages = handle.read_languages(timeout).ok()?;
    let lang = *languages.first()?;
    handle
        .read_serial_number_string(lang, desc, timeout)
        // it returns the serial number with the null termination after it, so remove it
        //
        // TODO: any better way?
        .map(|s| s.trim_matches('\0').to_string())
        .ok()
}

pub fn opaque_id(device: &rusb::Device<rusb::GlobalContext>) -> u64 {
    (device.bus_number() as u64) << 8 | device.address() as u64
}

pub(crate) fn device_endpoints(
    device: &rusb::Device<rusb::GlobalContext>,
    handle: Arc<rusb::DeviceHandle<rusb::GlobalContext>>,
) -> Result<(u8, u8, u16), RusbmuxError> {
    let desc = device.device_descriptor()?;

    let mut found_intf = None;
    let mut found_cfg_num = None;
    let mut in_addr = None;
    let mut out_addr = None;
    let mut out_max_packet_size = None;

    for n in 0..desc.num_configurations() {
        let cfg_desc = device.config_descriptor(n)?;

        for interface in cfg_desc.interfaces() {
            for alt in interface.descriptors() {
                if alt.class_code() == APPLE_USBMUX_CLASS
                    && alt.sub_class_code() == APPLE_USBMUX_SUBCLASS
                    && alt.protocol_code() == APPLE_USBMUX_PROTOCOL
                {
                    found_intf = Some(alt.interface_number());
                    found_cfg_num = Some(cfg_desc.number());
                    for ep in alt.endpoint_descriptors() {
                        match ep.direction() {
                            rusb::Direction::In => in_addr = Some(ep.address()),
                            rusb::Direction::Out => {
                                out_addr = Some(ep.address());
                                out_max_packet_size = Some(ep.max_packet_size());
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    let intf_num = found_intf.ok_or(RusbmuxError::UsbmuxInterfaceNotFound)?;
    let cfg_num = found_cfg_num.unwrap_or(1);
    let end_in = in_addr.ok_or(RusbmuxError::BulkInEndpointNotFound)?;
    let end_out = out_addr.ok_or(RusbmuxError::BulkOutEndpointNotFound)?;
    let max_packet_size = out_max_packet_size.unwrap_or(512);

    let active_cfg = handle.active_configuration().unwrap_or(0);
    if cfg_num != active_cfg || active_cfg == 0 {
        info!(
            old_cfg = active_cfg,
            new_cfg = cfg_num,
            "Switching device configuration"
        );

        #[cfg(target_os = "linux")]
        {
            if let Err(e) = handle.detach_kernel_driver(intf_num) {
                use tracing::warn;

                warn!(interface = intf_num, error = ?e, "Failed to detach kernel driver");
            } else {
                debug!(interface = intf_num, "Detached kernel driver");
            }
        }

        handle.set_active_configuration(cfg_num)?;
    }

    handle.claim_interface(intf_num)?;

    debug!(
        interface = intf_num,
        end_in, end_out, max_packet_size, "Claimed interface and endpoints"
    );

    Ok((end_in, end_out, max_packet_size))
}

pub struct RusbAsyncReader {
    handle: Arc<rusb::DeviceHandle<rusb::GlobalContext>>,
    endpoint: u8,
    buffer: Bytes,
    pos: usize,
    pending: Option<(TransferHandle, tokio::sync::oneshot::Receiver<BulkResult>)>,
}

impl RusbAsyncReader {
    pub fn new(handle: Arc<rusb::DeviceHandle<rusb::GlobalContext>>, endpoint: u8) -> Self {
        Self {
            handle,
            endpoint,
            buffer: Bytes::new(),
            pos: 0,
            pending: None,
        }
    }
}

impl AsyncRead for RusbAsyncReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        if this.pos < this.buffer.len() {
            let available = this.buffer.len() - this.pos;
            let to_copy = available.min(buf.remaining());
            buf.put_slice(&this.buffer[this.pos..this.pos + to_copy]);
            this.pos += to_copy;
            if this.pos == this.buffer.len() {
                this.buffer = Bytes::new();
                this.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }

        // drive any in-flight transfer
        if let Some((_, rx)) = &mut this.pending {
            return match Pin::new(rx).poll(cx) {
                Poll::Ready(Ok(result)) => {
                    if result.status != 0 {
                        this.pending = None;
                        return Poll::Ready(Err(io_error(format!(
                            "libusb read: {}",
                            libusb_status_str(result.status)
                        ))));
                    }
                    let chunk = Bytes::from(result.data);
                    chunk_to_buf(chunk, buf, &mut this.buffer, &mut this.pos);
                    this.pending = None;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(_)) => {
                    this.pending = None;
                    Poll::Ready(Err(io_error("read transfer cancelled")))
                }
                Poll::Pending => Poll::Pending,
            };
        }

        // no pending transfer, submit one
        {
            let buf_vec = vec![0u8; MAX_PACKET_PAYLOAD_SIZE * 2];
            match alloc_and_submit(&this.handle, this.endpoint, buf_vec) {
                Ok((handle, rx)) => this.pending = Some((handle, rx)),
                Err(e) => return Poll::Ready(Err(io_error(e.to_string()))),
            }
        }

        if let Some((_, rx)) = &mut this.pending {
            match Pin::new(rx).poll(cx) {
                Poll::Ready(Ok(result)) => {
                    let chunk = Bytes::from(result.data);
                    chunk_to_buf(chunk, buf, &mut this.buffer, &mut this.pos);
                    this.pending = None;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(_)) => {
                    this.pending = None;
                    Poll::Ready(Err(io_error("read transfer cancelled")))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Pending
        }
    }
}

fn chunk_to_buf(
    chunk: Bytes,
    buf: &mut tokio::io::ReadBuf<'_>,
    buffer: &mut Bytes,
    pos: &mut usize,
) {
    let to_copy = chunk.len().min(buf.remaining());
    buf.put_slice(&chunk[..to_copy]);
    if to_copy < chunk.len() {
        *buffer = chunk;
        *pos = to_copy;
    } else {
        buffer.clear();
        *pos = 0;
    }
}

pub struct RusbAsyncWriter {
    handle: Arc<rusb::DeviceHandle<rusb::GlobalContext>>,
    endpoint: u8,
    max_packet_size: usize,

    buffer: Vec<u8>,
    current_transfer_len: usize,

    pending: Option<(TransferHandle, tokio::sync::oneshot::Receiver<BulkResult>)>,
}

impl RusbAsyncWriter {
    pub fn new(
        handle: Arc<rusb::DeviceHandle<rusb::GlobalContext>>,
        endpoint: u8,
        max_packet_size: u16,
    ) -> Self {
        Self {
            handle,
            endpoint,
            max_packet_size: max_packet_size as usize,
            buffer: Vec::new(),
            current_transfer_len: 0,
            pending: None,
        }
    }

    fn sync_flush_and_zlp(&mut self) -> Result<(), RusbmuxError> {
        if !self.buffer.is_empty() {
            let data = std::mem::take(&mut self.buffer);
            self.current_transfer_len += data.len();
            self.handle
                .write_bulk(self.endpoint, &data, Duration::MAX)?;
        }

        if self.current_transfer_len > 0
            && self
                .current_transfer_len
                .is_multiple_of(self.max_packet_size)
        {
            self.handle.write_bulk(self.endpoint, &[], Duration::MAX)?;
        }

        self.current_transfer_len = 0;
        Ok(())
    }

    pub fn submit_end(&mut self) -> Result<(), RusbmuxError> {
        self.sync_flush_and_zlp()
    }
}

impl AsyncWrite for RusbAsyncWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();

        if let Some(pending) = this.pending.as_mut() {
            return match Pin::new(&mut pending.1).poll(cx) {
                Poll::Ready(Ok(result)) => {
                    if result.status != 0 {
                        return Poll::Ready(Err(io_error("transfer failed")));
                    }
                    Poll::Pending
                }
                Poll::Ready(Err(_)) => {
                    this.pending = None;
                    Poll::Ready(Err(io_error("write transfer cancelled")))
                }
                Poll::Pending => Poll::Pending,
            };
        }

        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let len = buf.len();
        this.buffer.extend_from_slice(buf);

        // auto flush above threshold
        if this.buffer.len() >= MAX_PACKET_SIZE * 2 {
            let data = std::mem::take(&mut this.buffer);
            match alloc_and_submit(&this.handle, this.endpoint, data) {
                Ok((h, rx)) => {
                    this.current_transfer_len += len;
                    this.pending = Some((h, rx));
                    return Poll::Pending;
                }
                Err(e) => return Poll::Ready(Err(io_error(e.to_string()))),
            }
        }

        Poll::Ready(Ok(len))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        // drive any in-flight transfer
        if let Some((_, rx)) = &mut this.pending {
            return match Pin::new(rx).poll(cx) {
                Poll::Ready(Ok(result)) => {
                    this.pending = None;
                    if result.status != 0 {
                        return Poll::Ready(Err(io_error(format!(
                            "libusb write: {}",
                            libusb_status_str(result.status)
                        ))));
                    }
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(_)) => {
                    this.pending = None;
                    Poll::Ready(Err(io_error("write transfer cancelled")))
                }
                Poll::Pending => Poll::Pending,
            };
        }

        if this.buffer.is_empty() {
            return Poll::Ready(Ok(()));
        }

        let data = std::mem::take(&mut this.buffer);
        let len = data.len();
        let pending = alloc_and_submit(&this.handle, this.endpoint, data);
        let (h, rx) = match pending {
            Ok(v) => v,
            Err(e) => return Poll::Ready(Err(io_error(e.to_string()))),
        };
        this.current_transfer_len += len;
        this.pending = Some((h, rx));

        if let Some((_, rx)) = &mut this.pending {
            match Pin::new(rx).poll(cx) {
                Poll::Ready(Ok(result)) => {
                    this.pending = None;
                    if result.status != 0 {
                        return Poll::Ready(Err(io_error(format!(
                            "libusb write: {}",
                            libusb_status_str(result.status)
                        ))));
                    }
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(_)) => {
                    this.pending = None;
                    Poll::Ready(Err(io_error("write transfer cancelled")))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }
}
