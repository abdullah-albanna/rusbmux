use bytes::Bytes;
use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use tokio::sync::watch;
use tracing::{debug, info, trace};

use crate::{
    error::RusbmuxError,
    parser::device_mux::{DeviceMuxPacket, TcpFlags},
};

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

    pub shutdown_rx: watch::Receiver<()>,
}

/// a place holder value,
///
/// it would rewritten by the writer loop to avoid the race condition on the seq
const AUTO_SEQ: u16 = 0;

impl DeviceMuxConn {
    /// # Safety
    ///
    /// make sure the connection is already opened and you took the values from the exact previous
    /// connection
    pub async unsafe fn new_from(
        device: Arc<Device>,
        destination_port: u16,
        source_port: u16,
        send_bytes: u32,
        recv_bytes: u32,
        rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
        tx: MAsyncTx<mpmc::Array<DeviceMuxPacket>>,
        shutdown_rx: watch::Receiver<()>,
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
            shutdown_rx,
        })
    }

    pub async fn new(
        device: Arc<Device>,
        destination_port: u16,
        rx: MAsyncRx<mpmc::Array<DeviceMuxPacket>>,
        tx: MAsyncTx<mpmc::Array<DeviceMuxPacket>>,
        shutdown_rx: watch::Receiver<()>,
    ) -> Result<Arc<Self>, RusbmuxError> {
        let source_port = device.get_next_source_port()?;
        let mut send_bytes = 0;
        let mut recv_bytes = 0;

        info!(
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

        tx.send(tcp_syn).await?;
        trace!(src = source_port, dst = destination_port, "Sent SYN");

        let tcp_syn_ack = rx.recv().await?;
        trace!(src = source_port, dst = destination_port, packet = ?tcp_syn_ack, "Received SYN-ACK");

        // if tcp_syn_ack.header.as_v2().unwrap().recv_seq.get()
        //     < tcp_syn.header.as_v2().unwrap().send_seq.get()
        // {
        //     panic!("device is behind or out-of-order");
        // }

        let tcp_syn_ack_tcp_hdr =
            tcp_syn_ack
                .tcp_hdr
                .as_ref()
                .ok_or(RusbmuxError::UnexpectedPacket(
                    "Expected a packet with a tcp header".to_string(),
                ))?;
        // should be 1 (syn)
        send_bytes += tcp_syn_ack_tcp_hdr.acknowledgment_number;

        // I've received 1 byte (syn-ack)
        recv_bytes += tcp_syn_ack_tcp_hdr.sequence_number;

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

        tx.send(tcp_ack).await?;
        trace!(src = source_port, dst = destination_port, "Sent ACK");

        info!(
            src = source_port,
            dst = destination_port,
            send_bytes,
            recv_bytes,
            "TCP handshake complete"
        );

        Ok(Arc::new(Self {
            device,
            sent_bytes: AtomicU32::new(send_bytes),
            recvd_bytes: AtomicU32::new(recv_bytes),
            source_port,
            destination_port,
            rx,
            tx,
            shutdown_rx,
        }))
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

    pub async fn send_bytes(&self, value: Bytes) -> Result<(), RusbmuxError> {
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

        self.send_packet(packet).await
    }

    pub async fn send_plist(&self, value: plist::Value) -> Result<(), RusbmuxError> {
        let packet = DeviceMuxPacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_recvd_bytes(),
                TcpFlags::ACK,
            )
            .payload_plist(value)
            .build();

        self.send_packet(packet).await
    }

    async fn send_packet(&self, packet: DeviceMuxPacket) -> Result<(), RusbmuxError> {
        let payload_len = packet.payload.len() as u32;

        self.tx.send(packet).await?;

        self.add_sent_bytes(payload_len);

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            len = payload_len,
            "Sent payload"
        );

        Ok(())
    }

    #[inline]
    pub async fn close(&self) -> Result<(), RusbmuxError> {
        debug!(
            src = self.source_port,
            dst = self.destination_port,
            "Closing connection"
        );

        self.send_rst().await
    }

    #[inline]
    pub fn close_blocking(&self) -> Result<(), RusbmuxError> {
        debug!(
            src = self.source_port,
            dst = self.destination_port,
            "Closing connection"
        );

        self.send_rst_blocking()
    }

    pub fn send_rst_blocking(&self) -> Result<(), RusbmuxError> {
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

        self.tx.try_send(rst_packet)?;

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            "Sent RST"
        );
        Ok(())
    }

    pub async fn send_rst(&self) -> Result<(), RusbmuxError> {
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

        self.tx.send(rst_packet).await?;

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            "Sent RST"
        );
        Ok(())
    }

    pub async fn ack(&self) -> Result<(), RusbmuxError> {
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

        self.tx.send(tcp_ack).await?;

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            "Sent ACK"
        );
        Ok(())
    }

    pub async fn recv(&self) -> Result<DeviceMuxPacket, RusbmuxError> {
        let response = self.rx.recv().await?;

        let recv_bytes = response.payload.len() as u32;

        self.recvd_bytes.store(
            response
                .tcp_hdr
                .as_ref()
                .map_or(recv_bytes, |t| t.sequence_number),
            std::sync::atomic::Ordering::Relaxed,
        );
        self.device.increment_recv_seq();

        self.ack().await?;
        Ok(response)
    }

    pub async fn wait_shutdown(&self) {
        self.shutdown_rx.clone().changed().await.unwrap();
    }
}
