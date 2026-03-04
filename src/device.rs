use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU16, AtomicU32},
    },
};

use bytes::Bytes;
use crossfire::{AsyncRx, AsyncTx, MAsyncRx, MAsyncTx, mpmc, mpsc, spsc};
use futures_lite::StreamExt;
use nusb::{
    Speed,
    hotplug::HotplugEvent,
    io::{EndpointRead, EndpointWrite},
    transfer::Bulk,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, OnceCell, RwLock, broadcast},
};

use crate::{
    parser::device_mux::{DeviceMuxPacket, DeviceMuxPayload, DeviceMuxVersion, TcpFlags},
    usb::{APPLE_VID, UsbStream, get_usb_endpoints, get_usbmux_interface},
    utils::nusb_speed_to_number,
};

/// a channel used for hotplug events, once a device is connected it get broadcasted to all it's
/// subscribers
///
/// it only sends basic information about the device, the device it self is stored in
/// `CONNECTED_DEVICES`
pub static HOTPLUG_EVENT_TX: OnceCell<broadcast::Sender<DeviceEvent>> = OnceCell::const_new();

/// has the currently connected devices with it's corresponding idevice id
///
/// devices are pushed to it whenever a device is connected, and removed once the device is removed
pub static CONNECTED_DEVICES: RwLock<Vec<Arc<Device>>> = RwLock::const_new(vec![]);

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Attached {
        serial_number: String,
        id: u64,
        speed: u64,
        product_id: u16,
        location_id: u32,
    },
    Detached {
        id: u64,
    },
}

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

    pub conns: RwLock<HashMap<u16, Arc<DeviceMuxConn>>>,
    pub conns_sender: RwLock<HashMap<u16, MAsyncTx<mpmc::Array<DeviceMuxPacket>>>>,
}

impl Device {
    pub async fn new(info: nusb::DeviceInfo, id: u64) -> Arc<Self> {
        let device_handle = info.open().await.unwrap();

        let usbmux_interface = get_usbmux_interface(&device_handle).await;
        let (mut end_in, mut end_out) = get_usb_endpoints(&device_handle, &usbmux_interface).await;

        let version_packet = DeviceMuxPacket::builder()
            .header_version()
            .payload_version(2, 0)
            .build();
        dbg!(&version_packet);

        end_out.write_all(&version_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        let version_response = DeviceMuxPacket::from_reader(&mut end_in).await;

        dbg!(&version_response);

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
            conns: RwLock::new(HashMap::new()),
            conns_sender: RwLock::new(HashMap::new()),
        });

        tokio::spawn(Self::start_reader_loop(Arc::clone(&device)));
        device
    }

    pub async fn start_reader_loop(self: Arc<Self>) {
        let mut end_in = self.end_in.lock().await;
        loop {
            let packet = DeviceMuxPacket::from_reader(&mut *end_in).await;

            let source_port = packet
                .tcp_hdr
                .as_ref()
                .map(|h| h.destination_port)
                .unwrap_or(0);

            loop {
                if let Some(tx) = self.conns_sender.read().await.get(&source_port) {
                    if tx.send(packet).await.is_err() {
                        self.conns_sender.write().await.remove(&source_port);
                    }
                    break;
                } else {
                    continue;
                }
            }
        }
    }

    pub async fn connect(self: &Arc<Self>, destination_port: u16) -> Arc<DeviceMuxConn> {
        let (tx, rx) = mpmc::bounded_async::<DeviceMuxPacket>(64);

        self.conns_sender.write().await.insert(
            self.next_source_port
                .load(std::sync::atomic::Ordering::Relaxed),
            tx,
        );

        let conn = DeviceMuxConn::new(Arc::clone(self), destination_port, rx).await;

        self.conns
            .write()
            .await
            .insert(conn.source_port, Arc::clone(&conn));

        conn
    }

    pub fn get_send_seq(&self) -> u16 {
        self.send_seq.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_recv_seq(&self) -> u16 {
        self.recv_seq.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn increment_send_seq(&self) {
        self.send_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        for (_, conn) in self.conns.read().await.iter() {
            conn.close().await;
        }
    }
}

pub struct DeviceMuxConn {
    pub device: Arc<Device>,
    pub sent_bytes: AtomicU32,
    pub recvd_bytes: AtomicU32,

    pub source_port: u16,
    pub destination_port: u16,

    pub rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
}

impl DeviceMuxConn {
    pub async fn new(
        device: Arc<Device>,
        destination_port: u16,
        rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
    ) -> Arc<Self> {
        let source_port = device.get_next_source_port();
        let mut send_bytes = 0;
        let mut recv_bytes = 0;

        let tcp_syn = DeviceMuxPacket::builder()
            .header_tcp(device.get_send_seq(), device.get_recv_seq())
            .tcp_header(
                source_port,
                destination_port,
                send_bytes,
                recv_bytes,
                TcpFlags::SYN,
            )
            .build();

        let mut end_out = device.as_ref().end_out.lock().await;

        end_out.write_all(&tcp_syn.encode()).await.unwrap();
        end_out.flush().await.unwrap();
        drop(end_out);

        let tcp_syn_ack = rx.recv().await.unwrap();

        dbg!(&tcp_syn_ack);

        assert_eq!(
            tcp_syn_ack.header.as_v2().unwrap().recv_seq.get(),
            device.get_send_seq()
        );

        device.increment_send_seq();

        // should be 1 (syn)
        send_bytes += tcp_syn_ack
            .tcp_hdr
            .expect("expected a tcp header")
            .acknowledgment_number;

        // I've received 1 byte (syn-ack)
        recv_bytes += 1;

        device.increment_recv_seq();

        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(device.get_send_seq(), device.get_recv_seq())
            .tcp_header(
                source_port,
                destination_port,
                send_bytes,
                recv_bytes,
                TcpFlags::ACK,
            )
            .build();

        let mut end_out = device.as_ref().end_out.lock().await;

        end_out.write_all(&tcp_ack.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        device.increment_send_seq();

        drop(end_out);

        Arc::new(Self {
            device,
            sent_bytes: AtomicU32::new(send_bytes),
            recvd_bytes: AtomicU32::new(recv_bytes),
            source_port,
            destination_port,
            rx,
        })
    }

    pub fn get_sent_bytes(&self) -> u32 {
        self.sent_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_recvd_bytes(&self) -> u32 {
        self.recvd_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn add_recvd_bytes(&self, value: u32) {
        self.recvd_bytes
            .fetch_add(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn add_sent_bytes(&self, value: u32) {
        self.sent_bytes
            .fetch_add(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub async fn send_bytes(&self, value: Bytes) {
        let packet = DeviceMuxPacket::builder()
            .header_tcp(self.device.get_send_seq(), self.device.get_recv_seq())
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::ACK,
            )
            .payload_raw(value)
            .build();

        self.send(&packet).await;
    }

    pub async fn close(&self) {
        self.send_rst().await;
    }

    pub async fn send_rst(&self) {
        let rst_packet = DeviceMuxPacket::builder()
            .header_tcp(self.device.get_send_seq(), self.device.get_recv_seq())
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::RST,
            )
            .build();

        self.device.increment_send_seq();

        let mut end_out = self.device.end_out.lock().await;

        end_out.write_all(&rst_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();
    }

    pub async fn ack(&self) {
        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(self.device.get_send_seq(), self.device.get_recv_seq())
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::ACK,
            )
            .build();

        let mut end_out = self.device.end_out.lock().await;

        end_out.write_all(&tcp_ack.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        self.device.increment_send_seq();
    }

    pub async fn recv(&self) -> DeviceMuxPacket {
        let response = self.rx.recv().await.unwrap();

        let recv_bytes = response.payload.as_raw().map_or(0, |b| b.len()) as u32;

        self.add_recvd_bytes(recv_bytes);
        self.device.increment_recv_seq();

        response
    }

    pub async fn send(&self, packet: &DeviceMuxPacket) {
        let mut end_out = self.device.end_out.lock().await;

        end_out
            .write_all(&packet.encode())
            .await
            .expect("unable to send a packet");
        end_out.flush().await.unwrap();

        self.device.increment_send_seq();
        self.add_sent_bytes(packet.payload.as_raw().map_or(0, |b| b.len()) as u32);
    }
}

/// get the currently connected devices and push them to the global `CONNECTED_DEVICES` with it's device
/// id
///
/// this is necessary because the hotplug event doesn't give back currently connected devices,
/// only fresh devices fresh
pub async fn push_currently_connected_devices(
    devices_id_map: &mut HashMap<nusb::DeviceId, u64>,
    device_id_counter: &mut u64,
) {
    let current_connected_devices = crate::usb::get_apple_device().await.collect::<Vec<_>>();

    if !current_connected_devices.is_empty() {
        let mut global_devices = CONNECTED_DEVICES.write().await;
        for device_info in current_connected_devices {
            devices_id_map.insert(device_info.id(), *device_id_counter);

            global_devices.push(Device::new(device_info, *device_id_counter).await);
            *device_id_counter += 1;
        }
    }
}

pub async fn remove_device(id: nusb::DeviceId) {
    let mut global_devices = CONNECTED_DEVICES.write().await;
    let device_idx = global_devices
        .iter()
        .position(|dev| dev.info.id() == id)
        .expect("unable to get the position of the about to be removed device");
    global_devices.remove(device_idx);
}
pub async fn device_watcher() {
    let hotplug_event_tx = HOTPLUG_EVENT_TX
        .get_or_init(|| async move { broadcast::channel::<DeviceEvent>(32).0 })
        .await;

    let mut devices_hotplug = nusb::watch_devices().unwrap().filter_map(|e| {
        // don't include the connected event if it's not an apple devices
        if matches!(&e, HotplugEvent::Connected(dev) if dev.vendor_id() != APPLE_VID) {
            return None;
        }

        Some(e)
    });

    let mut device_id_counter = 1;

    let mut devices_id_map = HashMap::new();

    push_currently_connected_devices(&mut devices_id_map, &mut device_id_counter).await;

    while let Some(event) = devices_hotplug.next().await {
        // // no one is listening
        // if hotplug_event_tx.receiver_count() < 1 {
        //     continue;
        // }

        println!("new event: {event:#?}");

        match event {
            HotplugEvent::Connected(device_info) => {
                devices_id_map.insert(device_info.id(), device_id_counter);

                let speed = nusb_speed_to_number(device_info.speed().unwrap_or(Speed::Low));

                let location_id =
                    (device_info.busnum() as u32) << 16 | device_info.device_address() as u32;

                if let Err(e) = hotplug_event_tx.send(DeviceEvent::Attached {
                    serial_number: device_info.serial_number().unwrap_or_default().to_string(),
                    id: device_id_counter,
                    speed,
                    product_id: device_info.product_id(),
                    location_id,
                }) {
                    eprintln!("looks like no one is listening, error: {e}");
                }

                CONNECTED_DEVICES
                    .write()
                    .await
                    .push(Device::new(device_info, device_id_counter).await);

                device_id_counter += 1;
            }
            HotplugEvent::Disconnected(device_id) => {
                // remove from both the global devices, and so as the id's map
                if let Some(id) = devices_id_map.remove(&device_id) {
                    remove_device(device_id).await;

                    if let Err(e) = hotplug_event_tx.send(DeviceEvent::Detached { id }) {
                        eprintln!("looks like no one is listening, error: {e}");
                    }
                }
            }
        }
    }
}
