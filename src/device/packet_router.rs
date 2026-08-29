use crossfire::{AsyncRx, AsyncTx, MAsyncRx, MAsyncTx, RecvFuture, flavor::Flavor, mpmc, spsc};
use dashmap::DashMap;
use tracing::{debug, trace, warn};

use crate::parser::device_mux::UsbDevicePacket;

#[derive(Debug)]
pub struct SAsyncPacketTx(pub AsyncTx<spsc::Array<UsbDevicePacket>>);

unsafe impl Sync for SAsyncPacketTx {}

#[derive(Debug)]
pub struct SAsyncPacketRx(pub AsyncRx<spsc::Array<UsbDevicePacket>>);

unsafe impl Sync for SAsyncPacketRx {}

#[derive(Debug)]
pub struct PacketRouter {
    pub conns: DashMap<u16, SAsyncPacketTx>,
}

impl Default for PacketRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketRouter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            conns: DashMap::new(),
        }
    }

    pub fn cleanup_dead(&self) {
        self.conns.retain(|port, conn| {
            let alive = !conn.0.is_disconnected();
            if !alive {
                debug!(port, "Removing dead connection");
            }
            alive
        });
    }

    pub fn register(&self, port: u16) -> SAsyncPacketRx {
        let (tx, rx) = spsc::bounded_async(256);

        self.conns.insert(port, SAsyncPacketTx(tx));

        debug!(port, "Connection registered");

        SAsyncPacketRx(rx)
    }

    #[inline]
    pub fn clear(&self) {
        self.conns.clear();
    }

    #[inline]
    pub fn unregister(&self, port: u16) {
        self.conns.remove(&port);
        debug!(port, "Connection unregistered");
    }

    pub async fn route(&self, packet: UsbDevicePacket) {
        let port = packet.tcp_hdr.as_ref().map_or(0, |h| h.destination_port);

        trace!(port, "Routing packet");

        if let Some(conn) = self.conns.get(&port) {
            if conn.0.send(packet).await.is_err() {
                warn!(port, "Connection dropped (receiver gone), unregistering");
                self.unregister(port);
            }
        } else {
            trace!(port, "No connection found, dropping packet");
        }
    }
}
