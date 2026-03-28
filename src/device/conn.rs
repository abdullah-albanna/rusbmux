use bytes::Bytes;
use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use tracing::{debug, trace};

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
    pub tx: MAsyncTx<mpmc::Array<DeviceMuxPacket>>,
}

/// a place holder value,
///
/// it would rewritten by the writer loop to avoid the race condition on the seq
const AUTO_SEQ: u16 = 0;

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
        tx: MAsyncTx<mpmc::Array<DeviceMuxPacket>>,
    ) -> Arc<Self> {
        debug!(
            src = source_port,
            dst = destination_port,
            send_bytes,
            recv_bytes,
            "Creating DeviceMuxConn from existing state"
        );
        Arc::new(Self {
            device,
            sent_bytes: AtomicU32::new(send_bytes),
            recvd_bytes: AtomicU32::new(recv_bytes),
            source_port,
            destination_port,
            rx,
            tx,
        })
    }

    pub async fn new(
        device: Arc<Device>,
        destination_port: u16,
        rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
        tx: MAsyncTx<mpmc::Array<DeviceMuxPacket>>,
    ) -> Arc<Self> {
        let source_port = device.get_next_source_port();
        let mut send_bytes = 0;
        let mut recv_bytes = 0;

        debug!(
            src = source_port,
            dst = destination_port,
            "Initiating TCP handshake"
        );

        let tcp_syn = DeviceMuxPacket::builder()
            // TODO: what if we opened two connections at the same time? would we get the same
            // seq?, is that a problem?
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                source_port,
                destination_port,
                send_bytes,
                recv_bytes,
                TcpFlags::SYN,
            )
            .build();

        tx.send(tcp_syn).await.unwrap();
        trace!(src = source_port, dst = destination_port, "Sent SYN");

        let tcp_syn_ack = rx.recv().await.unwrap();
        trace!(src = source_port, dst = destination_port, packet = ?tcp_syn_ack, "Received SYN-ACK");

        // if tcp_syn_ack.header.as_v2().unwrap().recv_seq.get()
        //     < tcp_syn.header.as_v2().unwrap().send_seq.get()
        // {
        //     panic!("device is behind or out-of-order");
        // }

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
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                source_port,
                destination_port,
                send_bytes,
                recv_bytes,
                TcpFlags::ACK,
            )
            .build();

        tx.send(tcp_ack).await.unwrap();
        trace!(src = source_port, dst = destination_port, "Sent ACK");

        debug!(
            src = source_port,
            dst = destination_port,
            send_bytes,
            recv_bytes,
            "TCP handshake complete"
        );

        Arc::new(Self {
            device,
            sent_bytes: AtomicU32::new(send_bytes),
            recvd_bytes: AtomicU32::new(recv_bytes),
            source_port,
            destination_port,
            rx,
            tx,
        })
    }

    #[inline]
    pub fn get_sent_bytes(&self) -> u32 {
        self.sent_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn get_recvd_bytes(&self) -> u32 {
        self.recvd_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn add_recvd_bytes(&self, value: u32) {
        self.recvd_bytes
            .fetch_add(value, std::sync::atomic::Ordering::Relaxed);

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            bytes = value,
            total = self.get_recvd_bytes(),
            "Updated received bytes"
        );
    }

    #[inline]
    pub fn add_sent_bytes(&self, value: u32) {
        self.sent_bytes
            .fetch_add(value, std::sync::atomic::Ordering::Relaxed);

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            bytes = value,
            total = self.get_sent_bytes(),
            "Updated sent bytes"
        );
    }

    pub async fn send_bytes(&self, value: Bytes) {
        let packet = DeviceMuxPacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::ACK,
            )
            .payload_bytes(value)
            .build();

        let payload_len = packet.payload.as_bytes().map_or(0, |b| b.len()) as u32;

        self.tx.send(packet).await.unwrap();

        self.add_sent_bytes(payload_len);

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            len = payload_len,
            "Sent payload bytes"
        );
    }

    #[inline]
    pub async fn close(&self) {
        debug!(
            src = self.source_port,
            dst = self.destination_port,
            "Closing connection"
        );

        self.send_rst().await;
    }

    pub async fn send_rst(&self) {
        let rst_packet = DeviceMuxPacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::RST,
            )
            .build();

        self.tx.send(rst_packet).await.unwrap();

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            "Sent RST"
        );
    }

    pub async fn ack(&self) {
        let tcp_ack = DeviceMuxPacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::ACK,
            )
            .build();

        self.tx.send(tcp_ack).await.unwrap();

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            "Sent ACK"
        );
    }

    pub async fn recv(&self) -> DeviceMuxPacket {
        let response = self.rx.recv().await.unwrap();

        let recv_bytes = response.payload.as_bytes().map_or(0, |b| b.len()) as u32;

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
}
