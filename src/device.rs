use std::collections::HashMap;

use futures_lite::StreamExt;
use nusb::{
    Speed,
    hotplug::HotplugEvent,
    io::{EndpointRead, EndpointWrite},
    transfer::Bulk,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{OnceCell, RwLock, broadcast},
};

use crate::{
    parser::{
        device_mux::{DeviceMuxPacket, DeviceMuxPayload, DeviceMuxVersion},
        device_mux_builder::TcpFlags,
    },
    usb::{APPLE_VID, get_usb_endpoints, get_usbmux_interface},
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
pub static CONNECTED_DEVICES: RwLock<Vec<Device>> = RwLock::const_new(vec![]);

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Attached {
        serial_number: String,
        id: u32,
        speed: u32,
        product_id: u16,
        device_address: u8,
    },
    Detached {
        id: u32,
    },
}

pub type SourcePort = u16;

#[derive(Debug)]
pub struct Device {
    pub inner: DeviceInner,
    pub conns: HashMap<SourcePort, DeviceMuxConn>,
}

pub struct DeviceInner {
    pub handler: nusb::Device,
    pub info: nusb::DeviceInfo,

    pub id: u32,

    pub sent_seq: u16,
    pub received_seq: u16,

    pub next_source_port: u16,

    pub version: DeviceMuxVersion,

    pub end_in: EndpointRead<Bulk>,
    pub end_out: EndpointWrite<Bulk>,
}

impl std::fmt::Debug for DeviceInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCore")
            .field("handle", &self.handler)
            .field("info", &self.info)
            .field("id", &self.id)
            .field("sent_seq", &self.sent_seq)
            .field("received_seq", &self.received_seq)
            .field("next_source_port", &self.next_source_port)
            .field("end_in", &"...")
            .field("end_out", &"...")
            .finish()
    }
}

impl Device {
    pub async fn new(info: nusb::DeviceInfo, id: u32) -> Self {
        let device_handle = info.open().await.unwrap();

        let usbmux_interface = get_usbmux_interface(&device_handle).await;
        let (end_out, end_in) = get_usb_endpoints(&device_handle, &usbmux_interface).await;

        let mut end_out = end_out.writer(512);
        let mut end_in = end_in.reader(512);

        let version_packet = DeviceMuxPacket::builder()
            .header_version()
            .payload_version(2, 0)
            .build();
        dbg!(&version_packet);

        end_out.write_all(&version_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        let version_response = DeviceMuxPacket::parse(&mut end_in).await;

        dbg!(&version_response);

        let DeviceMuxPayload::Version(version) = version_response.payload else {
            panic!("received non verison packet");
        };

        let setup_packet = DeviceMuxPacket::builder()
            .header_setup()
            .payload_raw(bytes::Bytes::from_static(&[0x07]))
            .build();

        end_out.write_all(&setup_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();

        Self {
            inner: DeviceInner {
                handler: device_handle,
                info,
                id,
                sent_seq: 1,
                received_seq: 0,
                next_source_port: 1,
                version,
                end_in,
                end_out,
            },
            conns: HashMap::new(),
        }
    }

    pub async fn connect(&mut self, destination_port: u16) {
        let conn = DeviceMuxConn::new(&mut self.inner, destination_port).await;

        self.conns.insert(conn.source_port, conn);
    }

    pub async fn close(self) {
        let mut inner = self.inner;

        for (_, conn) in self.conns {
            let rst_packet = DeviceMuxPacket::builder()
                .header_tcp(inner.sent_seq, inner.received_seq)
                .tcp_header(
                    conn.source_port,
                    conn.destination_port,
                    conn.sent_bytes,
                    conn.received_bytes,
                    TcpFlags::RST,
                )
                .build();

            inner.sent_seq += 1;

            inner.end_out.write_all(&rst_packet.encode()).await.unwrap();
            inner.end_out.flush().await.unwrap();
        }
    }
}

impl DeviceInner {
    pub fn get_next_source_port(&mut self) -> u16 {
        let source_port = self.next_source_port;

        self.next_source_port = self
            .next_source_port
            .checked_add(1)
            .expect("too much connections");

        source_port
    }
}

#[derive(Debug)]
pub struct DeviceMuxConn {
    pub sent_bytes: u32,
    pub received_bytes: u32,

    pub source_port: u16,
    pub destination_port: u16,
}

impl DeviceMuxConn {
    pub async fn new(dev: &mut DeviceInner, destination_port: u16) -> Self {
        let source_port = dev.get_next_source_port();
        let mut sent_bytes = 0;
        let mut received_bytes = 0;

        let tcp_syn = DeviceMuxPacket::builder()
            .header_tcp(dev.sent_seq, dev.received_seq)
            .tcp_header(
                source_port,
                destination_port,
                sent_bytes,
                received_bytes,
                TcpFlags::SYN,
            )
            .build();

        dbg!(&tcp_syn);

        dev.end_out.write_all(&tcp_syn.encode()).await.unwrap();
        dev.end_out.flush().await.unwrap();

        let tcp_syn_ack = DeviceMuxPacket::parse(&mut dev.end_in).await;

        dbg!(&tcp_syn_ack);
        assert_eq!(
            tcp_syn_ack.header.as_v2().unwrap().received_seq.get(),
            dev.sent_seq
        );

        dev.sent_seq += 1;

        // should be 1 (syn)
        sent_bytes += tcp_syn_ack
            .tcp_hdr
            .expect("expected a tcp header")
            .acknowledgment_number;

        // I've received 1 byte (syn-ack)
        received_bytes += 1;

        dev.received_seq += 1;

        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(dev.sent_seq, dev.received_seq)
            .tcp_header(
                source_port,
                destination_port,
                sent_bytes,
                received_bytes,
                TcpFlags::ACK,
            )
            .build();

        dev.end_out.write_all(&tcp_ack.encode()).await.unwrap();
        dev.end_out.flush().await.unwrap();

        dev.sent_seq += 1;

        Self {
            sent_bytes,
            received_bytes,
            source_port,
            destination_port,
        }
    }
}

/// get the currently connected devices and push them to the global `CONNECTED_DEVICES` with it's device
/// id
///
/// this is necessary because the hotplug event doesn't give back currently connected devices,
/// only fresh devices fresh
pub async fn push_currently_connected_devices(
    devices_id_map: &mut HashMap<nusb::DeviceId, u32>,
    device_id_counter: &mut u32,
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
        .position(|dev| dev.inner.info.id() == id)
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

    let mut device_id_counter = 0;

    let mut devices_id_map = HashMap::new();

    push_currently_connected_devices(&mut devices_id_map, &mut device_id_counter).await;

    while let Some(event) = devices_hotplug.next().await {
        // no one is listening
        if hotplug_event_tx.receiver_count() < 1 {
            continue;
        }

        println!("new event: {event:#?}");

        match event {
            HotplugEvent::Connected(device_info) => {
                devices_id_map.insert(device_info.id(), device_id_counter);

                let speed = nusb_speed_to_number(device_info.speed().unwrap_or(Speed::Low));

                if let Err(e) = hotplug_event_tx.send(DeviceEvent::Attached {
                    serial_number: device_info.serial_number().unwrap_or_default().to_string(),
                    id: device_id_counter,
                    speed,
                    product_id: device_info.product_id(),
                    device_address: device_info.device_address(),
                }) {
                    eprintln!("looks like no one is listening, error: {e}")
                }

                let mut global_devices = CONNECTED_DEVICES.write().await;
                global_devices.push(Device::new(device_info, device_id_counter).await);

                device_id_counter += 1;
            }
            HotplugEvent::Disconnected(device_id) => {
                // remove from both the global devices, and so as the id's map
                if let Some(id) = devices_id_map.remove(&device_id) {
                    remove_device(device_id).await;

                    if let Err(e) = hotplug_event_tx.send(DeviceEvent::Detached { id }) {
                        eprintln!("looks like no one is listening, error: {e}")
                    }
                }
            }
        }
    }
}
