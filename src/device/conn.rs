use bytes::Bytes;
use crossfire::{MAsyncRx, mpmc};
use tokio::io::AsyncWriteExt;

use crate::parser::device_mux::{DeviceMuxPacket, TcpFlags};

use super::Device;
use std::sync::{Arc, atomic::AtomicU32};

pub struct DeviceMuxConn {
    pub device: Arc<Device>,
    pub sent_bytes: AtomicU32,
    pub recvd_bytes: AtomicU32,

    pub source_port: u16,
    pub destination_port: u16,

    pub rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
}

impl DeviceMuxConn {
    /// # Safety
    ///
    /// make sure the connection is already opened
    pub async unsafe fn new_from(
        device: Arc<Device>,
        destination_port: u16,
        source_port: u16,
        send_bytes: u32,
        recv_bytes: u32,
        rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            device,
            sent_bytes: AtomicU32::new(send_bytes),
            recvd_bytes: AtomicU32::new(recv_bytes),
            source_port,
            destination_port,
            rx,
        })
    }

    pub async fn new(
        device: Arc<Device>,
        destination_port: u16,
        rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
    ) -> Arc<Self> {
        let source_port = device.get_next_source_port();
        let mut send_bytes = 0;
        let mut recv_bytes = 0;

        let tcp_syn = DeviceMuxPacket::builder()
            // TODO: what if we opened two connections at the same time? would we get the same
            // seq?, is that a problem?
            .header_tcp(device.take_send_seq(), device.get_recv_seq())
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
            tcp_syn.header.as_v2().unwrap().send_seq.get()
        );

        // should be 1 (syn)
        send_bytes += tcp_syn_ack
            .tcp_hdr
            .as_ref()
            .expect("expected a tcp header")
            .acknowledgment_number;

        // I've received 1 byte (syn-ack)
        recv_bytes += tcp_syn_ack.tcp_hdr.as_ref().unwrap().sequence_number;

        device.increment_recv_seq();

        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(device.take_send_seq(), device.get_recv_seq())
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
            .header_tcp(self.device.take_send_seq(), self.device.get_recv_seq())
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
            .header_tcp(self.device.take_send_seq(), self.device.get_recv_seq())
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::RST,
            )
            .build();

        let mut end_out = self.device.end_out.lock().await;

        end_out.write_all(&rst_packet.encode()).await.unwrap();
        end_out.flush().await.unwrap();
    }

    pub async fn ack(&self) {
        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(self.device.take_send_seq(), self.device.get_recv_seq())
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
    }

    pub async fn recv(&self) -> DeviceMuxPacket {
        let response = self.rx.recv().await.unwrap();

        let recv_bytes = response.payload.as_raw().map_or(0, |b| b.len()) as u32;

        self.recvd_bytes.store(
            response
                .tcp_hdr
                .as_ref()
                .map_or(recv_bytes, |t| t.sequence_number),
            std::sync::atomic::Ordering::Relaxed,
        );
        self.device.increment_recv_seq();

        self.ack().await;
        response
    }

    pub async fn send(&self, packet: &DeviceMuxPacket) {
        let mut end_out = self.device.end_out.lock().await;

        end_out
            .write_all(&packet.encode())
            .await
            .expect("unable to send a packet");
        end_out.flush().await.unwrap();

        self.add_sent_bytes(packet.payload.as_raw().map_or(0, |b| b.len()) as u32);
    }
}
