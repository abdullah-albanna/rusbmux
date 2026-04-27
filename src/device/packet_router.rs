use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use dashmap::DashMap;
use tracing::{debug, trace, warn};

use crate::parser::device_mux::UsbDevicePacket;

#[derive(Debug)]
pub struct PacketRouter {
    pub conns: DashMap<u16, MAsyncTx<mpmc::Array<UsbDevicePacket>>>,
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
            let alive = !conn.is_disconnected();
            if !alive {
                debug!(port, "Removing dead connection");
            }
            alive
        });
    }

    pub fn register(&self, port: u16) -> MAsyncRx<mpmc::Array<UsbDevicePacket>> {
        let (tx, rx) = mpmc::bounded_async(16);

        self.conns.insert(port, tx);

        debug!(port, "Connection registered");

        rx
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
            if conn.send(packet).await.is_err() {
                warn!(port, "Connection dropped (receiver gone), unregistering");
                self.unregister(port);
            }
        } else {
            trace!(port, "No connection found, dropping packet");
        }
    }
}
