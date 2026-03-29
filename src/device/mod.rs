use std::sync::{Arc, atomic::AtomicU16};
pub mod conn;

use arc_swap::ArcSwapOption;
use bytes::{Bytes, BytesMut};
use conn::DeviceMuxConn;
use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use nusb::{
    io::{EndpointRead, EndpointWrite},
    transfer::Bulk,
};
use pack1::U16BE;
use tokio::{
    io::{AsyncWriteExt, BufReader},
    sync::{OnceCell, watch},
    task::JoinHandle,
};
use tracing::{debug, error, info, trace};

use crate::{
    error::{ParseError, RusbmuxError},
    packet_router::PacketRouter,
    parser::device_mux::{DeviceMuxHeader, DeviceMuxPacket, DeviceMuxPayload, DeviceMuxVersion},
    usb::{get_usb_endpoints, get_usbmux_interface},
};

pub struct Device {
    pub handler: nusb::Device,
    pub info: nusb::DeviceInfo,

    pub id: u64,

    pub send_seq: AtomicU16,
    pub recv_seq: AtomicU16,

    pub next_source_port: AtomicU16,

    pub version: DeviceMuxVersion,

    pub w_tx: MAsyncTx<mpmc::Array<DeviceMuxPacket>>,

    pub router: Arc<PacketRouter>,
    pub conns: Box<[ArcSwapOption<DeviceMuxConn>]>,

    pub shutdown_tx: watch::Sender<()>,
    pub shutdown_rx: watch::Receiver<()>,

    reader_loop_handler: OnceCell<JoinHandle<()>>,
    writer_loop_handler: OnceCell<JoinHandle<()>>,
}

impl Device {
    /// # Safety
    ///
    /// make sure you already sent the `DeviceMuxProtocol::Setup` packet
    pub async unsafe fn new_from(
        info: nusb::DeviceInfo,
        id: u64,
        version: DeviceMuxVersion,
    ) -> Result<Arc<Self>, RusbmuxError> {
        debug!(device_id = id, "Creating device from existing state");
        let device_handle = info.open().await?;

        let usbmux_interface = get_usbmux_interface(&device_handle).await?;
        let (end_in, end_out) = get_usb_endpoints(&device_handle, &usbmux_interface).await?;

        let mut vec = Vec::with_capacity(65536);
        for _ in 0..65536 {
            vec.push(ArcSwapOption::const_empty());
        }

        let (tx, rx) = mpmc::bounded_async(128);
        let (shutdown_tx, shutdown_rx) = watch::channel(());

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            id,
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            w_tx: tx,
            conns: vec.into_boxed_slice(),
            router: Arc::new(PacketRouter::new()),
            shutdown_tx,
            shutdown_rx,
            reader_loop_handler: OnceCell::const_new(),
            writer_loop_handler: OnceCell::const_new(),
        });

        info!(device_id = id, "Spawning reader & writer loops");

        let reader_loop_handler = tokio::spawn(Self::start_reader_loop(
            Arc::clone(&device.router),
            BufReader::new(end_in),
            id,
        ));
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

        let version_packet = DeviceMuxPacket::builder()
            .header_version()
            .payload_version(2, 0)
            .build();

        end_out.write_all(&version_packet.encode()).await?;
        end_out.flush().await?;

        debug!(device_id = id, "Sent version packet");

        let version_response = DeviceMuxPacket::from_reader(&mut end_in).await?;

        let version = match version_response.payload {
            DeviceMuxPayload::Version(v) => v,
            _ => {
                return Err(RusbmuxError::UnexpectedPacket(
                    "Expected verison packet".to_string(),
                ));
            }
        };

        debug!(device_id = id, version = ?version, "Received version response");

        let setup_packet = DeviceMuxPacket::builder()
            .header_setup()
            .payload_bytes(Bytes::from_static(&[0x07]))
            .build();

        end_out.write_all(&setup_packet.encode()).await?;
        end_out.flush().await?;

        debug!(device_id = id, "Sent setup packet");

        let mut vec = Vec::with_capacity(65536);
        for _ in 0..65536 {
            vec.push(ArcSwapOption::const_empty());
        }

        let (tx, rx) = mpmc::bounded_async(128);
        let (shutdown_tx, shutdown_rx) = watch::channel(());

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            id,
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            w_tx: tx,
            conns: vec.into_boxed_slice(),
            router: Arc::new(PacketRouter::new()),
            shutdown_tx,
            shutdown_rx,
            reader_loop_handler: OnceCell::const_new(),
            writer_loop_handler: OnceCell::const_new(),
        });

        info!(device_id = id, "Spawning reader & writer loops");

        let reader_loop_handler = tokio::spawn(Self::start_reader_loop(
            Arc::clone(&device.router),
            BufReader::new(end_in),
            id,
        ));
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
        router: Arc<PacketRouter>,
        mut end_in: BufReader<EndpointRead<Bulk>>,
        device_id: u64,
    ) {
        info!(target: "device_reader", device_id, "Reader loop started");
        loop {
            let packet = match DeviceMuxPacket::from_reader(&mut end_in).await {
                Ok(p) => p,

                // if it's an io, then the device probably got disconnected
                Err(ParseError::IO(e)) => {
                    error!(target: "device_reader", device_id, err = ?e, "Failed to read packet");
                    break;
                }

                Err(e) => {
                    error!(target: "device_reader", device_id, err = ?e, "Failed to read packet");
                    continue;
                }
            };

            debug!(
                target: "device_reader",
                device_id,
                port = packet.tcp_hdr.as_ref().map(|h| h.destination_port),
                payload = ?packet.payload.as_bytes(),
                "Packet received"
            );

            if let Some(t) = packet.tcp_hdr.as_ref()
                && t.rst
            {
                error!(
                    target: "device_reader",
                    device_id,
                    port = t.destination_port,
                    payload = ?packet.payload.as_bytes(),
                    "Received TCP RST"
                );
            }

            router.route(packet).await;
        }
    }

    pub async fn start_writer_loop(
        self: Arc<Self>,
        rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
        mut end_out: EndpointWrite<Bulk>,
        device_id: u64,
    ) {
        let mut buf = BytesMut::with_capacity(40 * 1024);

        info!(target: "device_writer", device_id, "Writer loop started");
        loop {
            let mut packet = match rx.recv().await {
                Ok(p) => p,
                Err(_) => {
                    error!(target: "device_writer", device_id, "Writer channel closed");
                    break;
                }
            };

            debug!(
                target: "device_writer",
                device_id,
                port = packet.tcp_hdr.as_ref().map(|h| h.destination_port),
                payload = ?packet.payload.as_bytes(),
                "Packet received"
            );

            if let DeviceMuxHeader::V2(v2) = &mut packet.header {
                let send_seq = self.take_send_seq();
                let recv_seq = self.get_recv_seq();

                trace!(target: "device_writer", device_id, send_seq, recv_seq, "Updating seq numbers");

                v2.send_seq = U16BE::new(send_seq);
                v2.recv_seq = U16BE::new(recv_seq);
            }

            buf.clear();
            packet.encode_into(&mut buf);

            trace!(target: "device_writer", device_id, len = buf.len(), "Encoded packet, writing...");

            if let Err(e) = end_out.write_all(&buf[..]).await {
                error!(target: "device_writer", device_id, err = ?e, "Failed to write packet");
            }

            if let Err(e) = end_out.flush().await {
                error!(target: "device_writer", device_id, err = ?e, "Failed to flush packet");
            } else {
                trace!(target: "device_writer", device_id, "Packet flushed");
            }
        }
    }

    pub async fn connect(
        self: &Arc<Self>,
        destination_port: u16,
    ) -> Result<Arc<DeviceMuxConn>, RusbmuxError> {
        let source_port = self
            .next_source_port
            .load(std::sync::atomic::Ordering::Relaxed);

        debug!(
            device_id = self.id,
            src_port = source_port,
            dst_port = destination_port,
            "Creating new connection"
        );

        let rx = self.router.register(source_port);

        let conn = DeviceMuxConn::new(
            Arc::clone(self),
            destination_port,
            rx,
            self.w_tx.clone(),
            self.shutdown_rx.clone(),
        )
        .await?;

        unsafe {
            self.conns
                .get_unchecked(conn.source_port as usize)
                .store(Some(Arc::clone(&conn)));
        };

        Ok(conn)
    }

    /// # Safety
    ///
    /// make sure the connection is already opened
    pub async unsafe fn connect_from(
        self: &Arc<Self>,
        destination_port: u16,
        source_port: u16,
        send_bytes: u32,
        recv_bytes: u32,
    ) -> Arc<DeviceMuxConn> {
        debug!(
            device_id = self.id,
            src_port = source_port,
            dst_port = destination_port,
            "Connecting from existing state"
        );

        let rx = self.router.register(source_port);

        let conn = unsafe {
            DeviceMuxConn::new_from(
                Arc::clone(self),
                destination_port,
                source_port,
                send_bytes,
                recv_bytes,
                rx,
                self.w_tx.clone(),
                self.shutdown_rx.clone(),
            )
            .await
        };

        unsafe {
            self.conns
                .get_unchecked(conn.source_port as usize)
                .store(Some(Arc::clone(&conn)));
        };

        conn
    }

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
        debug!(device_id = self.id, "Closing all connections");
        for conn in &self.conns {
            if let Some(c) = conn.load_full() {
                c.close().await?;
            }
        }

        Ok(())
    }

    pub fn close_all_blocking(&self) -> Result<(), RusbmuxError> {
        debug!(device_id = self.id, "Closing all connections");
        for conn in &self.conns {
            if let Some(c) = conn.load_full() {
                c.close_blocking()?;
            }
        }

        Ok(())
    }

    fn drop_loops(&self) {
        if let Some(rh) = self.reader_loop_handler.get() {
            debug!(device_id = self.id, "Aborting reader loop");
            rh.abort();
        }

        if let Some(wh) = self.writer_loop_handler.get() {
            debug!(device_id = self.id, "Aborting writer loop");
            wh.abort();
        }
    }

    pub async fn shutdown(&self) -> Result<(), RusbmuxError> {
        self.close_all().await?;
        self.drop_loops();
        self.shutdown_tx.send(())?;

        Ok(())
    }

    pub fn shutdown_blocking(&self) -> Result<(), RusbmuxError> {
        self.close_all_blocking()?;
        self.drop_loops();
        self.shutdown_tx.send(())?;

        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.shutdown_blocking();
    }
}
