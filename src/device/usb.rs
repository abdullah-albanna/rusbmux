use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU16},
};

use bytes::Bytes;
use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use dashmap::DashMap;
use etherparse::TcpHeader;
use nusb::{
    Speed,
    io::{EndpointRead, EndpointWrite},
    transfer::Bulk,
};
use pack1::U16BE;
use tokio::{io::AsyncWriteExt, sync::OnceCell, task::JoinHandle};
use tracing::{debug, error, info, trace, warn};

use crate::{
    conn::UsbDeviceConn,
    device::{core::DeviceCore, packet_router::PacketRouter},
    error::{ParseError, RusbmuxError},
    parser::device_mux::{
        UsbDevicePacket, UsbDevicePacketHeader, UsbDevicePacketHeaderV2, UsbDevicePacketPayload,
        UsbDevicePacketVersion,
    },
    usb::{get_usb_endpoints, get_usbmux_interface},
    utils::{self, nusb_speed_to_number},
};

#[derive(Debug)]
pub struct UsbDevice {
    pub handler: nusb::Device,
    pub info: nusb::DeviceInfo,

    pub core: DeviceCore,

    pub send_seq: AtomicU16,
    pub recv_seq: AtomicU16,

    pub next_source_port: AtomicU16,

    pub version: UsbDevicePacketVersion,

    pub w_tx: MAsyncTx<mpmc::Array<UsbDevicePacket>>,

    pub router: Arc<PacketRouter>,
    pub conns: DashMap<u16, Weak<UsbDeviceConn>>,

    reader_loop_handler: OnceCell<JoinHandle<()>>,
    writer_loop_handler: OnceCell<JoinHandle<()>>,

    dropped: AtomicBool,
}

impl UsbDevice {
    /// # Safety
    ///
    /// make sure you already sent the `UsbDevicePacketProtocol::Setup` packet
    pub async unsafe fn new_from(
        info: nusb::DeviceInfo,
        id: u64,
        version: UsbDevicePacketVersion,
    ) -> Result<Arc<Self>, RusbmuxError> {
        debug!(device_id = id, "Creating device from existing state");
        let device_handle = info.open().await?;

        let usbmux_interface = get_usbmux_interface(&device_handle).await?;
        let (end_in, end_out) = get_usb_endpoints(&device_handle, &usbmux_interface).await?;

        let (tx, rx) = mpmc::bounded_async(16);

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            core: DeviceCore::new(id),
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            w_tx: tx,
            conns: DashMap::new(),
            router: Arc::new(PacketRouter::new()),
            reader_loop_handler: OnceCell::const_new(),
            writer_loop_handler: OnceCell::const_new(),
            dropped: AtomicBool::new(false),
        });

        info!(device_id = id, "Spawning reader & writer loops");

        let reader_loop_handler =
            tokio::spawn(Self::start_reader_loop(Arc::clone(&device), end_in, id));
        let writer_loop_handler = tokio::spawn(Self::start_writer_loop(
            Arc::clone(&device),
            rx,
            end_out,
            id,
        ));

        device.reader_loop_handler.set(reader_loop_handler).unwrap();
        device.writer_loop_handler.set(writer_loop_handler).unwrap();

        debug!(device_id = id, "Device created");

        Ok(device)
    }

    pub async fn new(info: nusb::DeviceInfo, id: u64) -> Result<Arc<Self>, RusbmuxError> {
        debug!(device_id = id, "Creating new device");
        let device_handle = info.open().await?;

        let usbmux_interface = get_usbmux_interface(&device_handle).await?;
        let (mut end_in, mut end_out) =
            get_usb_endpoints(&device_handle, &usbmux_interface).await?;

        let version_packet = UsbDevicePacket::builder()
            .header_version()
            .payload_version(2, 0)
            .build();

        end_out.write_all(&version_packet.encode()).await?;
        end_out.flush().await?;

        debug!(device_id = id, "Sent version packet");

        let version = loop {
            let version_response = UsbDevicePacket::from_reader(&mut end_in).await?;

            match version_response.payload {
                UsbDevicePacketPayload::Version(v) => break v,
                _ => {
                    debug!("Received a non version packet, dropping");
                    continue;
                }
            }
        };

        debug!(device_id = id, version = ?version, "Received version response");

        let setup_packet = UsbDevicePacket::builder()
            .header_setup()
            .payload_bytes(Bytes::from_static(&[0x07]))
            .build();

        end_out.write_all(&setup_packet.encode()).await?;
        end_out.flush().await?;

        debug!(device_id = id, "Sent setup packet");

        let (tx, rx) = mpmc::bounded_async(16);

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            core: DeviceCore::new(id),
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            w_tx: tx,
            conns: DashMap::new(),
            router: Arc::new(PacketRouter::new()),
            reader_loop_handler: OnceCell::const_new(),
            writer_loop_handler: OnceCell::const_new(),
            dropped: AtomicBool::new(false),
        });

        info!(device_id = id, "Spawning reader & writer loops");

        let reader_loop_handler =
            tokio::spawn(Self::start_reader_loop(Arc::clone(&device), end_in, id));
        let writer_loop_handler = tokio::spawn(Self::start_writer_loop(
            Arc::clone(&device),
            rx,
            end_out,
            id,
        ));

        device.reader_loop_handler.set(reader_loop_handler).unwrap();
        device.writer_loop_handler.set(writer_loop_handler).unwrap();

        debug!(device_id = id, "Device created");

        Ok(device)
    }

    pub async fn start_reader_loop(
        self: Arc<Self>,
        mut end_in: EndpointRead<Bulk>,
        device_id: u64,
    ) {
        end_in.set_num_transfers(3);
        info!(target: "device_reader", device_id, "Reader loop started");
        loop {
            trace!(target: "device_reader", device_id, "Waiting for a packet");
            let packet = match UsbDevicePacket::from_reader(&mut end_in).await {
                Ok(p) => p,

                // if it's an io, then the device probably got disconnected
                Err(ParseError::IO(e)) => {
                    warn!(target: "device_reader", device_id, err = ?e, "Failed to read packet");
                    break;
                }

                Err(e) => {
                    error!(target: "device_reader", device_id, err = ?e, "Failed to read packet");
                    continue;
                }
            };

            self.increment_recv_seq();

            if let Some(t) = packet.tcp_hdr.as_ref()
                && t.rst
            {
                error!(
                    target: "device_reader",
                    device_id,
                    port = t.source_port,
                    payload = ?packet.payload.as_bytes(),
                    "Received TCP RST"
                );
                continue;
            } else if let UsbDevicePacketPayload::Error {
                error_code,
                message,
            } = &packet.payload
            {
                error!(
                    target: "device_reader",
                    device_id,
                    tcp_hdr = ?packet.tcp_hdr,
                    error_code = ?error_code,
                    message = ?message,
                    "Received an error packet"
                );
                self.router.route(packet).await;
                continue;
            }

            debug!(
                target: "device_reader",
                device_id,
                payload = ?packet.payload.as_bytes(),
                len = packet.header.as_v2().unwrap().length.get(),
                "Received a packet from the device"
            );

            self.router.route(packet).await;
        }
    }

    pub async fn start_writer_loop(
        self: Arc<Self>,
        rx: MAsyncRx<mpmc::Array<UsbDevicePacket>>,
        mut end_out: EndpointWrite<Bulk>,
        device_id: u64,
    ) {
        end_out.set_num_transfers(3);
        let mut hbuf = [0; UsbDevicePacketHeaderV2::SIZE + TcpHeader::MIN_LEN];

        info!(target: "device_writer", device_id, "Writer loop started");
        loop {
            trace!(target: "device_writer", device_id, "Waiting for a packet");
            let Ok(mut packet) = rx.recv().await else {
                error!(target: "device_writer", device_id, "Writer channel closed");
                break;
            };

            debug!(
                target: "device_writer",
                device_id,
                payload = ?packet.payload.as_bytes(),
                "Received a packet from the client"
            );

            if let UsbDevicePacketHeader::V2(v2) = &mut packet.header {
                let send_seq = self.take_send_seq();
                let recv_seq = self.get_recv_seq();

                trace!(target: "device_writer", device_id, send_seq, recv_seq, "Updating seq numbers");

                v2.send_seq = U16BE::new(send_seq);
                v2.recv_seq = U16BE::new(recv_seq);
            }

            trace!(target: "device_writer", device_id, "Encoding headers");
            match packet.header {
                UsbDevicePacketHeader::V1(h) => {
                    if let Err(e) = end_out.write_all(h.encode()).await {
                        error!(target: "device_writer", device_id, err = ?e, "Failed to write packet header v1");
                    }
                }
                UsbDevicePacketHeader::V2(h) => {
                    hbuf[..UsbDevicePacketHeaderV2::SIZE].copy_from_slice(h.encode());

                    if let Some(tcp_hdr) = packet.tcp_hdr.as_ref() {
                        hbuf[UsbDevicePacketHeaderV2::SIZE..].copy_from_slice(&tcp_hdr.to_bytes());

                        if let Err(e) = end_out.write_all(&hbuf).await {
                            error!(target: "device_writer", device_id, err = ?e, "Failed to write packet header v2");
                        }
                    } else if let Err(e) = end_out
                        .write_all(&hbuf[..UsbDevicePacketHeaderV2::SIZE])
                        .await
                    {
                        error!(target: "device_writer", device_id, err = ?e, "Failed to write packet header v2");
                    }
                }
            }

            let payload = packet.payload.encode();

            trace!(target: "device_writer", device_id, len = payload.len(), "Writing payload");

            if let Err(e) = end_out.write_all(&payload).await {
                error!(target: "device_writer", device_id, err = ?e, "Failed to write packet payload");
            }

            end_out.submit_end();
        }
    }

    pub async fn connect(
        self: &Arc<Self>,
        destination_port: u16,
    ) -> Result<Arc<UsbDeviceConn>, RusbmuxError> {
        let source_port = self
            .next_source_port
            .load(std::sync::atomic::Ordering::Relaxed);

        debug!(
            device_id = self.core.id,
            src_port = source_port,
            dst_port = destination_port,
            "Creating new connection"
        );

        let rx = self.router.register(source_port);

        let conn = UsbDeviceConn::new(
            self,
            Arc::downgrade(&Arc::clone(&self.router)),
            destination_port,
            rx,
            self.w_tx.clone(),
        )
        .await?;

        self.conns
            .insert(conn.source_port, Arc::downgrade(&Arc::clone(&conn)));

        Ok(conn)
    }

    /// # Safety
    ///
    /// make sure the connection is already opened
    pub unsafe fn connect_from(
        self: &Arc<Self>,
        destination_port: u16,
        source_port: u16,
        sent_bytes: u32,
        received_bytes: u32,
        device_last_window_size: u16,
        device_last_received_bytes: u32,
    ) -> Arc<UsbDeviceConn> {
        debug!(
            device_id = self.core.id,
            src_port = source_port,
            dst_port = destination_port,
            "Connecting from existing state"
        );

        let rx = self.router.register(source_port);

        let conn = unsafe {
            UsbDeviceConn::new_from(
                self,
                Arc::downgrade(&Arc::clone(&self.router)),
                destination_port,
                source_port,
                sent_bytes,
                received_bytes,
                device_last_window_size,
                device_last_received_bytes,
                rx,
                self.w_tx.clone(),
            )
        };

        self.conns
            .insert(conn.source_port, Arc::downgrade(&Arc::clone(&conn)));

        conn
    }

    pub async fn cleanup_conn(&self, conn: &UsbDeviceConn) -> Result<(), RusbmuxError> {
        let source_port = conn.source_port;

        if let Some((_, conn)) = self.conns.remove(&source_port)
            && let Some(conn) = conn.upgrade()
            && !conn.dropped()
        {
            conn.close().await?;
        }

        self.router.unregister(source_port);

        Ok(())
    }

    #[inline]
    pub fn get_next_source_port(&self) -> Result<u16, RusbmuxError> {
        match self
            .next_source_port
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        {
            0 => Err(RusbmuxError::RanOutofSourcePort),
            sp => Ok(sp),
        }
    }

    pub async fn close_all(&self) -> Result<(), RusbmuxError> {
        debug!(device_id = self.core.id, "Closing all connections");
        for conn in self.conns.iter().filter_map(|c| c.upgrade()) {
            conn.send_rst().await?;
        }

        self.router.clear();
        self.conns.clear();

        Ok(())
    }

    pub fn close_all_blocking(&self) -> Result<(), RusbmuxError> {
        debug!(device_id = self.core.id, "Closing all connections");
        for conn in self.conns.iter().filter_map(|c| c.upgrade()) {
            conn.send_rst_blocking()?;
        }

        self.router.clear();
        self.conns.clear();

        Ok(())
    }

    #[inline]
    fn dropped(&self) -> bool {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    fn set_dropped(&self) {
        self.dropped
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn drop_loops(&self) {
        if let Some(rh) = self.reader_loop_handler.get() {
            debug!(device_id = self.core.id, "Aborting reader loop");
            rh.abort();
        }

        if let Some(wh) = self.writer_loop_handler.get() {
            debug!(device_id = self.core.id, "Aborting writer loop");
            wh.abort();
        }
    }

    pub async fn shutdown(&self) -> Result<(), RusbmuxError> {
        self.set_dropped();
        self.close_all().await?;
        self.drop_loops();
        self.core.canceler.cancel();

        Ok(())
    }

    pub fn shutdown_blocking(&self) -> Result<(), RusbmuxError> {
        self.set_dropped();
        self.close_all_blocking()?;
        self.drop_loops();
        self.core.canceler.cancel();

        Ok(())
    }
}

impl UsbDevice {
    #[inline]
    pub fn take_send_seq(&self) -> u16 {
        self.send_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn get_recv_seq(&self) -> u16 {
        self.recv_seq.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn increment_recv_seq(&self) {
        self.recv_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        if !self.dropped() {
            let _ = self.shutdown_blocking();
        }
    }
}

impl UsbDevice {
    pub fn create_device_attached(&self) -> Result<plist::Value, RusbmuxError> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let location_id = (self.info.busnum() as u32) << 16 | self.info.device_address() as u32;

        #[cfg(target_os = "macos")]
        let location_id = self.info.location_id();

        let speed = nusb_speed_to_number(self.info.speed().unwrap_or(Speed::Low));
        let serial_number = utils::get_serial_number(&self.info).to_string();

        debug!(
            device_id = self.core.id,
            serial_number,
            speed,
            location_id,
            product_id = self.info.product_id(),
            "Adding device to plist"
        );

        Ok(plist_macro::plist!({
            "DeviceID": self.core.id,
            "MessageType": "Attached",
            "Properties": {
                "ConnectionSpeed": speed,
                "ConnectionType": "USB",
                "DeviceID": self.core.id,
                "LocationID": location_id,
                "ProductID": self.info.product_id(),
                "SerialNumber": serial_number,
            }
        }))
    }
}
