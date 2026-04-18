use std::sync::Arc;

use arc_swap::ArcSwapOption;
use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use tracing::{debug, trace, warn};

use crate::parser::device_mux::UsbDevicePacket;

pub struct PacketRouter {
    connections: Box<[ArcSwapOption<MAsyncTx<mpmc::Array<UsbDevicePacket>>>]>,
}

impl Default for PacketRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketRouter {
    #[must_use]
    pub fn new() -> Self {
        let mut vec = Vec::with_capacity(65536);
        for _ in 0..65536 {
            vec.push(ArcSwapOption::const_empty());
        }

        Self {
            connections: vec.into_boxed_slice(),
        }
    }

    pub fn register(&self, port: u16) -> MAsyncRx<mpmc::Array<UsbDevicePacket>> {
        let (tx, rx) = mpmc::bounded_async(128);

        self.connections[port as usize].store(Some(Arc::new(tx)));

        debug!(port, "Connection registered");

        rx
    }

    #[inline]
    pub fn unregister(&self, port: u16) {
        self.connections[port as usize].store(None);
        debug!(port, "Connection unregistered");
    }

    pub async fn route(&self, packet: UsbDevicePacket) {
        let port = packet.tcp_hdr.as_ref().map_or(0, |h| h.destination_port);

        trace!(port, "Routing packet");

        if let Some(conn) = self.connections[port as usize].load_full() {
            if conn.send(packet).await.is_err() {
                warn!(port, "Connection dropped (receiver gone), unregistering");
                self.unregister(port);
            }
        } else {
            trace!(port, "No connection found, dropping packet");
        }
    }
}
