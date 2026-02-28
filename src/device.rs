use std::collections::{HashMap, VecDeque};

use bytes::{Buf, BufMut, Bytes, BytesMut};
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
    parser::device_mux::{DeviceMuxPacket, DeviceMuxPayload, DeviceMuxVersion, TcpFlags},
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
        id: u64,
        speed: u32,
        product_id: u16,
        device_address: u8,
    },
    Detached {
        id: u64,
    },
}

pub type SourcePort = u16;

#[derive(Debug)]
pub struct Device {
    pub inner: DeviceInner,
    pub conns: HashMap<SourcePort, DeviceMuxConn>,
    pub recvd_buff: VecDeque<DeviceMuxPacket>,
}

pub struct DeviceInner {
    pub handler: nusb::Device,
    pub info: nusb::DeviceInfo,

    pub id: u64,

    pub send_seq: u16,
    pub recv_seq: u16,

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
            .field("send_seq", &self.send_seq)
            .field("recv_seq", &self.recv_seq)
            .field("next_source_port", &self.next_source_port)
            .field("version", &self.version)
            .field("end_in", &"...")
            .field("end_out", &"...")
            .finish()
    }
}

impl Device {
    pub async fn new(info: nusb::DeviceInfo, id: u64) -> Self {
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

        Self {
            inner: DeviceInner {
                handler: device_handle,
                info,
                id,
                send_seq: 1,
                recv_seq: 0,
                next_source_port: 1,
                version,
                end_in,
                end_out,
            },
            conns: HashMap::new(),
            recvd_buff: VecDeque::new(),
        }
    }

    pub async fn connect(&mut self, destination_port: u16) {
        let conn = DeviceMuxConn::new(&mut self.inner, destination_port).await;

        self.conns.insert(conn.source_port, conn);
    }

    pub async fn send(&mut self, value: plist::Value, port: u16) -> DeviceMuxPacket {
        let conn = self
            .conns
            .entry(port)
            .or_insert(DeviceMuxConn::new(&mut self.inner, port).await);

        let packet = DeviceMuxPacket::builder()
            .header_tcp(self.inner.send_seq, self.inner.recv_seq)
            .tcp_header(
                conn.source_port,
                conn.destination_port,
                conn.sent_bytes,
                conn.recvd_bytes,
                TcpFlags::ACK,
            )
            .payload_plist(value)
            .build();

        let (response, mut other_packets) = conn.send(&mut self.inner, &packet).await;

        self.recvd_buff.append(&mut other_packets);

        response
    }

    pub async fn close(self) {
        let mut inner = self.inner;

        for (_, conn) in self.conns {
            let rst_packet = DeviceMuxPacket::builder()
                .header_tcp(inner.send_seq, inner.recv_seq)
                .tcp_header(
                    conn.source_port,
                    conn.destination_port,
                    conn.sent_bytes,
                    conn.recvd_bytes,
                    TcpFlags::RST,
                )
                .build();

            inner.send_seq += 1;

            inner.end_out.write_all(&rst_packet.encode()).await.unwrap();
            inner.end_out.flush().await.unwrap();
        }
    }
}

impl DeviceInner {
    pub const fn get_next_source_port(&mut self) -> u16 {
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
    pub recvd_bytes: u32,

    pub source_port: u16,
    pub destination_port: u16,
}

impl DeviceMuxConn {
    pub async fn new(dev: &mut DeviceInner, destination_port: u16) -> Self {
        let source_port = dev.get_next_source_port();
        let mut send_bytes = 0;
        let mut recv_bytes = 0;

        let tcp_syn = DeviceMuxPacket::builder()
            .header_tcp(dev.send_seq, dev.recv_seq)
            .tcp_header(
                source_port,
                destination_port,
                send_bytes,
                recv_bytes,
                TcpFlags::SYN,
            )
            .build();

        dbg!(&tcp_syn);

        dev.end_out.write_all(&tcp_syn.encode()).await.unwrap();
        dev.end_out.flush().await.unwrap();

        let tcp_syn_ack = DeviceMuxPacket::from_reader(&mut dev.end_in).await;

        dbg!(&tcp_syn_ack);
        assert_eq!(
            tcp_syn_ack.header.as_v2().unwrap().recv_seq.get(),
            dev.send_seq
        );

        dev.send_seq += 1;

        // should be 1 (syn)
        send_bytes += tcp_syn_ack
            .tcp_hdr
            .expect("expected a tcp header")
            .acknowledgment_number;

        // I've received 1 byte (syn-ack)
        recv_bytes += 1;

        dev.recv_seq += 1;

        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(dev.send_seq, dev.recv_seq)
            .tcp_header(
                source_port,
                destination_port,
                send_bytes,
                recv_bytes,
                TcpFlags::ACK,
            )
            .build();

        dev.end_out.write_all(&tcp_ack.encode()).await.unwrap();
        dev.end_out.flush().await.unwrap();

        dev.send_seq += 1;

        Self {
            sent_bytes: send_bytes,
            recvd_bytes: recv_bytes,
            source_port,
            destination_port,
        }
    }

    pub async fn ack(&mut self, dev: &mut DeviceInner) {
        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(dev.send_seq, dev.recv_seq)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.sent_bytes,
                self.recvd_bytes,
                TcpFlags::ACK,
            )
            .build();

        dev.end_out.write_all(&tcp_ack.encode()).await.unwrap();
        dev.end_out.flush().await.unwrap();

        dev.send_seq += 1;
    }

    pub async fn recv_one(&mut self, dev: &mut DeviceInner) -> DeviceMuxPacket {
        let response = DeviceMuxPacket::from_reader(&mut dev.end_in).await;

        let recv_bytes = response.payload.as_raw().map_or(0, |b| b.len()) as u32;

        self.recvd_bytes += recv_bytes;
        dev.recv_seq += 1;

        response
    }

    pub async fn recv(
        &mut self,
        dev: &mut DeviceInner,
        port: u16,
        others: &mut VecDeque<DeviceMuxPacket>,
    ) -> DeviceMuxPacket {
        // keep receiving until we get the first packet of our port
        let first_response = loop {
            let response = self.recv_one(dev).await;

            if let Some(tcp) = response.tcp_hdr.as_ref()
                && tcp.source_port == port
            {
                break response;
            }

            others.push_back(response);
        };

        let res_len = first_response.header.get_length() as usize;

        // we'll assume that we can return it
        // if it's an empty payload or it's either a version payload or an error that
        //
        // TODO: might be a problem
        if res_len == DeviceMuxPacket::HEADERS_LEN_V2
            || matches!(
                first_response.payload,
                DeviceMuxPayload::Version(_) | DeviceMuxPayload::Error { .. }
            )
        {
            self.ack(dev).await;
            return first_response;
        }

        // there is payload
        if res_len > DeviceMuxPacket::HEADERS_LEN_V2
            && let DeviceMuxPayload::Raw(first_response_payload) = &first_response.payload
        {
            let full_response_len = match first_response_payload.len() {
                // probably got only the length prefix
                4 => first_response_payload.slice(..).get_u32(),
                // got less than the length prefix, I'll ignore it for now,
                //
                // TODO: read more packets until you get the 4 bytes
                1..=3 => unimplemented!("splitted length prefix is not implemented yet"),
                // big enough
                _ => first_response_payload.slice(..4).get_u32(),
            } as usize;

            // we got the full packet from the first response
            if full_response_len + 4 == first_response_payload.len() {
                self.ack(dev).await;
                return first_response;
            }

            let mut amount_received: usize = 0;
            let mut collected_splitted_packets = vec![first_response];

            loop {
                let next_response = self.recv_one(dev).await;

                // skip packets thats not ours
                if next_response
                    .tcp_hdr
                    .as_ref()
                    .is_none_or(|tcp| tcp.source_port != port)
                {
                    others.push_back(next_response);
                    continue;
                }

                amount_received += next_response.get_payload_len();

                collected_splitted_packets.push(next_response);

                // we got all of them
                if amount_received == full_response_len {
                    break;
                }
            }

            let collected_payload =
                collected_splitted_packets
                    .iter()
                    .fold(BytesMut::new(), |mut b, p| {
                        b.put(p.payload.as_raw().cloned().unwrap());
                        b
                    });

            let mut last_recvd_packet =
                collected_splitted_packets.remove(collected_splitted_packets.len() - 1);

            last_recvd_packet.payload = DeviceMuxPayload::Raw(collected_payload.freeze());

            self.ack(dev).await;
            last_recvd_packet
        } else {
            panic!("received broken packet")
        }
    }

    /// sends a packets and receives YOUR full response, and the packets that got caught in between
    pub async fn send(
        &mut self,
        dev: &mut DeviceInner,
        packet: &DeviceMuxPacket,
    ) -> (DeviceMuxPacket, VecDeque<DeviceMuxPacket>) {
        let mut others = VecDeque::new();

        dev.end_out
            .write_all(&packet.encode())
            .await
            .expect("unable to send a packet");
        dev.end_out.flush().await.unwrap();

        dev.send_seq += 1;
        self.sent_bytes += packet.payload.as_raw().map_or(0, |b| b.len()) as u32;

        // TODO: can the port be connected twice or more, thus having more than one response?
        let port = packet
            .tcp_hdr
            .as_ref()
            .expect("sent packet must be tcp")
            .destination_port;

        let response = self.recv(dev, port, &mut others).await;

        (response, others)
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
