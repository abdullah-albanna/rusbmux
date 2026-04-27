use bytes::Bytes;
use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use tracing::{debug, info, trace};

use crate::{
    device::{core::DeviceCore, packet_router::PacketRouter, usb::UsbDevice},
    error::RusbmuxError,
    parser::device_mux::{TcpFlags, UsbDevicePacket},
    usb::MAX_PACKET_PAYLOAD_SIZE,
};

use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicUsize},
};

#[derive(Debug)]
pub struct UsbDeviceConn {
    pub device_core: DeviceCore,
    pub device_router: Weak<PacketRouter>,
    pub sent_bytes: AtomicU32,
    pub received_bytes: AtomicU32,

    pub source_port: u16,
    pub destination_port: u16,

    pub sendable_bytes: AtomicUsize,

    pub device_last_window_size: AtomicU16,
    pub device_last_received_bytes: AtomicU32,

    pub rx: MAsyncRx<mpmc::Array<UsbDevicePacket>>,
    pub tx: MAsyncTx<mpmc::Array<UsbDevicePacket>>,

    dropped: AtomicBool,
}

/// a place holder value,
///
/// it would be rewritten by the writer loop to avoid the race condition on the seq
const AUTO_SEQ: u16 = 0;

impl UsbDeviceConn {
    pub const WINDOW_SIZE: u16 = ((128u32 * 1024) >> 8) as u16;

    /// # Safety
    ///
    /// make sure the connection is already opened and you took the values from the exact previous
    /// connection
    //
    //  TODO: do a dump states method and let this take that
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn new_from(
        device: &UsbDevice,
        device_router: Weak<PacketRouter>,
        destination_port: u16,
        source_port: u16,
        sent_bytes: u32,
        received_bytes: u32,
        device_last_window_size: u16,
        device_last_received_bytes: u32,
        rx: MAsyncRx<mpmc::Array<UsbDevicePacket>>,
        tx: MAsyncTx<mpmc::Array<UsbDevicePacket>>,
    ) -> Arc<Self> {
        debug!(
            src = source_port,
            dst = destination_port,
            sent_bytes,
            received_bytes,
            "Creating UsbDeviceConn from existing state"
        );
        Arc::new(Self {
            device_core: device.core.clone(),
            device_router,
            sent_bytes: AtomicU32::new(sent_bytes),
            received_bytes: AtomicU32::new(received_bytes),
            source_port,
            destination_port,
            sendable_bytes: AtomicUsize::new(Self::calc_sendable_bytes(
                sent_bytes,
                device_last_window_size,
                device_last_received_bytes,
            )),
            device_last_window_size: AtomicU16::new(device_last_window_size),
            device_last_received_bytes: AtomicU32::new(device_last_received_bytes),
            rx,
            tx,
            dropped: AtomicBool::new(false),
        })
    }

    pub async fn new(
        device: &UsbDevice,
        device_router: Weak<PacketRouter>,
        destination_port: u16,
        rx: MAsyncRx<mpmc::Array<UsbDevicePacket>>,
        tx: MAsyncTx<mpmc::Array<UsbDevicePacket>>,
    ) -> Result<Arc<Self>, RusbmuxError> {
        let source_port = device.get_next_source_port()?;
        let mut sent_bytes = 0;
        let mut received_bytes = 0;

        info!(
            src = source_port,
            dst = destination_port,
            "Initiating TCP handshake"
        );

        let tcp_syn = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                source_port,
                destination_port,
                sent_bytes,
                received_bytes,
                TcpFlags::SYN,
            )
            .build();

        // let tcp_syn_header = tcp_syn.header;

        tx.send(tcp_syn).await?;
        trace!(src = source_port, dst = destination_port, "Sent SYN");

        let tcp_syn_ack = rx.recv().await?;
        debug!(
            src = source_port,
            dst = destination_port,
            "Received SYN-ACK"
        );

        // let our_send_seq = tcp_syn_header.as_v2().unwrap().send_seq.get();
        // let device_recv_seq = tcp_syn_ack.header.as_v2().map(|h| h.recv_seq.get());
        //
        // debug_assert!(
        //     device_recv_seq.is_some_and(|seq| seq < our_send_seq),
        //     "device is behind or out-of-order"
        // );

        let tcp_syn_ack_tcp_hdr =
            tcp_syn_ack
                .tcp_hdr
                .as_ref()
                .ok_or(RusbmuxError::UnexpectedPacket(
                    "Expected a packet with a tcp header".to_string(),
                ))?;
        // should be 1 (syn)
        sent_bytes += tcp_syn_ack_tcp_hdr.acknowledgment_number;

        // I've received 1 byte (syn-ack)
        received_bytes += tcp_syn_ack_tcp_hdr.sequence_number;

        let tcp_ack = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                source_port,
                destination_port,
                sent_bytes,
                received_bytes,
                TcpFlags::ACK,
            )
            .build();

        tx.send(tcp_ack).await?;

        trace!(src = source_port, dst = destination_port, "Sent ACK");

        let last_device_window_size = tcp_syn_ack_tcp_hdr.window_size;
        let device_received_bytes = tcp_syn_ack_tcp_hdr.acknowledgment_number;

        let sendable_bytes =
            Self::calc_sendable_bytes(sent_bytes, last_device_window_size, device_received_bytes);

        info!(
            src = source_port,
            dst = destination_port,
            sent_bytes,
            received_bytes,
            "TCP handshake complete"
        );

        Ok(Arc::new(Self {
            device_core: device.core.clone(),
            device_router,
            sent_bytes: AtomicU32::new(sent_bytes),
            received_bytes: AtomicU32::new(received_bytes),
            source_port,
            destination_port,
            sendable_bytes: AtomicUsize::new(sendable_bytes),
            device_last_window_size: AtomicU16::new(last_device_window_size),
            device_last_received_bytes: AtomicU32::new(device_received_bytes),
            rx,
            tx,
            dropped: AtomicBool::new(false),
        }))
    }

    /// you must include the length prefix at the start
    pub async fn send_bytes(&self, value: Bytes) -> Result<(), RusbmuxError> {
        let packet = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_received_bytes(),
                TcpFlags::ACK,
            )
            .payload_bytes(value)
            .build();

        self.send_packet(packet).await
    }

    pub async fn send_plist(&self, value: plist::Value) -> Result<(), RusbmuxError> {
        let packet = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_received_bytes(),
                TcpFlags::ACK,
            )
            .payload_plist(value)
            .build();

        self.send_packet(packet).await
    }

    async fn send_packet(&self, packet: UsbDevicePacket) -> Result<(), RusbmuxError> {
        let payload_len = packet.payload.len() as u32;

        self.tx.send(packet).await?;

        self.add_sent_bytes(payload_len);

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            len = payload_len,
            "Sent payload"
        );

        self.update_sendable_bytes();

        Ok(())
    }

    #[inline]
    pub async fn close(&self) -> Result<(), RusbmuxError> {
        self.set_dropped();
        self.send_rst().await?;

        if let Some(router) = self.device_router.upgrade() {
            router.unregister(self.source_port);
        }

        Ok(())
    }

    #[inline]
    pub fn close_blocking(&mut self) -> Result<(), RusbmuxError> {
        self.set_dropped();
        self.send_rst_blocking()?;

        if let Some(router) = self.device_router.upgrade() {
            router.unregister(self.source_port);
        }

        Ok(())
    }

    pub fn send_rst_blocking(&self) -> Result<(), RusbmuxError> {
        debug!(
            src = self.source_port,
            dst = self.destination_port,
            "Closing connection"
        );

        let rst_packet = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_received_bytes(),
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
        debug!(
            src = self.source_port,
            dst = self.destination_port,
            "Closing connection"
        );

        let rst_packet = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_received_bytes(),
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
        let tcp_ack = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.get_sent_bytes(),
                self.get_received_bytes(),
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

    pub async fn recv(&self) -> Result<UsbDevicePacket, RusbmuxError> {
        let response = self.rx.recv().await?;

        let recv_bytes = response.payload.len() as u32;
        let tcp_hdr = response.tcp_hdr.as_ref();

        self.set_received_bytes(tcp_hdr.map_or(recv_bytes, |t| t.sequence_number));

        if let Some(h) = tcp_hdr {
            self.set_device_last_received_bytes(h.acknowledgment_number);
            self.set_device_last_window_size(h.window_size);
        }

        self.ack().await?;

        self.update_sendable_bytes();

        Ok(response)
    }

    pub fn update_sendable_bytes(&self) {
        let sendable = Self::calc_sendable_bytes(
            self.get_sent_bytes(),
            self.get_device_last_window_size(),
            self.get_device_last_received_bytes(),
        );

        self.set_sendable_bytes(sendable);

        trace!("new sendable amount of bytes: {sendable}",);
    }

    fn calc_sendable_bytes(
        sent_bytes: u32,
        device_window_size: u16,
        device_received_bytes: u32,
    ) -> usize {
        // the device right shifts the window size (so are we), so put it back to get the actual
        // value
        let device_window_size = (device_window_size as u32) << 8;

        let unacked_bytes = sent_bytes.saturating_sub(device_received_bytes);

        if device_window_size > unacked_bytes {
            ((device_window_size - unacked_bytes) as usize).min(MAX_PACKET_PAYLOAD_SIZE)
        } else {
            0
        }
    }

    pub async fn wait_shutdown(&self) -> Result<(), RusbmuxError> {
        self.device_core.canceler.cancelled().await;
        Ok(())
    }
}

// atomic setters and getters
impl UsbDeviceConn {
    #[inline]
    pub fn get_sent_bytes(&self) -> u32 {
        self.sent_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn get_received_bytes(&self) -> u32 {
        self.received_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn get_sendable_bytes(&self) -> usize {
        self.sendable_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn set_sendable_bytes(&self, value: usize) {
        self.sendable_bytes
            .store(value, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    pub fn get_device_last_window_size(&self) -> u16 {
        self.device_last_window_size
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn set_device_last_window_size(&self, value: u16) {
        self.device_last_window_size
            .store(value, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    pub fn get_device_last_received_bytes(&self) -> u32 {
        self.device_last_received_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn set_device_last_received_bytes(&self, value: u32) {
        self.device_last_received_bytes
            .store(value, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    pub fn set_received_bytes(&self, value: u32) {
        self.received_bytes
            .store(value, std::sync::atomic::Ordering::Relaxed);

        trace!(
            src = self.source_port,
            dst = self.destination_port,
            bytes = value,
            total = self.get_received_bytes(),
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

    #[inline]
    pub fn dropped(&self) -> bool {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    pub fn set_dropped(&self) {
        self.dropped
            .store(true, std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for UsbDeviceConn {
    fn drop(&mut self) {
        if !self.dropped() {
            let _ = self.close_blocking();
        }
    }
}
