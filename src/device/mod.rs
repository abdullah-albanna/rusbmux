use std::sync::{Arc, atomic::AtomicU16};
mod conn;

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use conn::DeviceMuxConn;
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

    pub end_out: Mutex<EndpointWrite<Bulk>>,

    pub router: PacketRouter,
    pub conns: Box<[ArcSwapOption<DeviceMuxConn>]>,
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

        let mut vec = Vec::with_capacity(65536);
        for _ in 0..65536 {
            vec.push(ArcSwapOption::const_empty());
        }

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            id,
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            end_out: Mutex::new(end_out),
            conns: vec.into_boxed_slice(),
            router: PacketRouter::new(),
        });

        tokio::spawn(Self::start_reader_loop(Arc::clone(&device), end_in));
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
            .payload_bytes(Bytes::from_static(&[0x07]))
            .build();

        end_out.write_all(&setup_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        let mut vec = Vec::with_capacity(65536);
        for _ in 0..65536 {
            vec.push(ArcSwapOption::const_empty());
        }

        let device = Arc::new(Self {
            handler: device_handle,
            info,
            id,
            send_seq: AtomicU16::new(1),
            recv_seq: AtomicU16::new(0),
            next_source_port: AtomicU16::new(1),
            version,
            end_out: Mutex::new(end_out),
            conns: vec.into_boxed_slice(),
            router: PacketRouter::new(),
        });

        tokio::spawn(Self::start_reader_loop(Arc::clone(&device), end_in));
        device
    }

    // pub async fn start_reader_loop(self: Arc<Self>, mut end_in: EndpointRead<Bulk>) {
    //     loop {
    //         let mut buf = end_in.fill_buf().await.unwrap();
    //
    //         if buf.is_empty() {
    //             continue;
    //         }
    //
    //         let mut total_consumed = 0;
    //
    //         while !buf.is_empty() {
    //             let start_len = buf.len();
    //
    //             if buf.len() < DeviceMuxHeaderV1::SIZE {
    //                 break;
    //             }
    //
    //             let mut tmp = buf;
    //             let header = DeviceMuxHeader::from_slice(&mut tmp);
    //             let total_len = header.get_length() as usize;
    //
    //             if start_len < total_len {
    //                 break;
    //             }
    //
    //             let mut packet_slice = &buf[..total_len];
    //             let packet = DeviceMuxPacket::from_slice(&mut packet_slice);
    //
    //             buf = &buf[total_len..];
    //             total_consumed += total_len;
    //
    //             self.router.route(packet).await;
    //         }
    //
    //         end_in.consume(total_consumed);
    //     }
    // }

    pub async fn start_reader_loop(self: Arc<Self>, mut end_in: EndpointRead<Bulk>) {
        loop {
            let packet = DeviceMuxPacket::from_reader(&mut end_in).await;

            self.router.route(packet).await;
        }
    }

    pub async fn connect(self: &Arc<Self>, destination_port: u16) -> Arc<DeviceMuxConn> {
        let rx = self.router.register(
            self.next_source_port
                .load(std::sync::atomic::Ordering::Relaxed),
        );

        let conn = DeviceMuxConn::new(Arc::clone(self), destination_port, rx).await;

        self.conns[conn.source_port as usize].store(Some(Arc::clone(&conn)));

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

        self.conns[conn.source_port as usize].store(Some(Arc::clone(&conn)));

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
    pub fn get_next_source_port(&self) -> u16 {
        // TODO: handle overflow
        self.next_source_port
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn close_all(&self) {
        for conn in &self.conns {
            if let Some(c) = conn.load_full() {
                c.close().await;
            }
        }
    }
}
