use std::sync::{Arc, atomic::AtomicU16};
mod conn;

use bytes::Bytes;
use conn::DeviceMuxConn;
use dashmap::DashMap;
use nusb::{
    io::{EndpointRead, EndpointWrite},
    transfer::Bulk,
};
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::{
    packet_router::PacketRouter,
    parser::device_mux::{DeviceMuxPacket, DeviceMuxPayload, DeviceMuxVersion},
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

    pub end_in: Mutex<EndpointRead<Bulk>>,
    pub end_out: Mutex<EndpointWrite<Bulk>>,

    pub router: PacketRouter,
    pub conns: DashMap<u16, Arc<DeviceMuxConn>>,
}

impl Device {
    /// # Safety
    ///
    /// make sure you already sent the `DeviceMuxProtocol::Setup` packet
    pub async unsafe fn new_from(
        info: nusb::DeviceInfo,
        id: u64,
        version: DeviceMuxVersion,
    ) -> Arc<Self> {
        let device_handle = info.open().await.unwrap();

        let usbmux_interface = get_usbmux_interface(&device_handle).await;
        let (end_in, end_out) = get_usb_endpoints(&device_handle, &usbmux_interface).await;

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            id,
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            end_in: Mutex::new(end_in),
            end_out: Mutex::new(end_out),
            conns: DashMap::new(),
            router: PacketRouter::new(),
        });

        tokio::spawn(Self::start_reader_loop(Arc::clone(&device)));
        device
    }

    pub async fn new(info: nusb::DeviceInfo, id: u64) -> Arc<Self> {
        let device_handle = info.open().await.unwrap();

        let usbmux_interface = get_usbmux_interface(&device_handle).await;
        let (mut end_in, mut end_out) = get_usb_endpoints(&device_handle, &usbmux_interface).await;

        let version_packet = DeviceMuxPacket::builder()
            .header_version()
            .payload_version(2, 0)
            .build();

        end_out.write_all(&version_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        let version_response = DeviceMuxPacket::from_reader(&mut end_in).await;

        let DeviceMuxPayload::Version(version) = version_response.payload else {
            panic!("received non verison packet");
        };

        let setup_packet = DeviceMuxPacket::builder()
            .header_setup()
            .payload_raw(Bytes::from_static(&[0x07]))
            .build();

        end_out.write_all(&setup_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            id,
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            end_in: Mutex::new(end_in),
            end_out: Mutex::new(end_out),
            conns: DashMap::new(),
            router: PacketRouter::new(),
        });

        tokio::spawn(Self::start_reader_loop(Arc::clone(&device)));
        device
    }

    pub async fn start_reader_loop(self: Arc<Self>) {
        let mut end_in = self.end_in.lock().await;
        loop {
            let packet = DeviceMuxPacket::from_reader(&mut *end_in).await;

            self.router.route(packet).await;
        }
    }

    pub async fn connect(self: &Arc<Self>, destination_port: u16) -> Arc<DeviceMuxConn> {
        let rx = self.router.register(
            self.next_source_port
                .load(std::sync::atomic::Ordering::Relaxed),
        );

        let conn = DeviceMuxConn::new(Arc::clone(self), destination_port, rx).await;

        self.conns.insert(conn.source_port, Arc::clone(&conn));

        conn
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
        let rx = self.router.register(source_port);

        let conn = unsafe {
            DeviceMuxConn::new_from(
                Arc::clone(self),
                destination_port,
                source_port,
                send_bytes,
                recv_bytes,
                rx,
            )
            .await
        };

        self.conns.insert(conn.source_port, Arc::clone(&conn));

        conn
    }

    pub fn take_send_seq(&self) -> u16 {
        self.send_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_recv_seq(&self) -> u16 {
        self.recv_seq.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn increment_recv_seq(&self) {
        self.recv_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_next_source_port(&self) -> u16 {
        // TODO: handle overflow
        self.next_source_port
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn close_all(&self) {
        for conn in &self.conns {
            conn.close().await;
        }
    }
}
