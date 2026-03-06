use crossfire::{MAsyncRx, MAsyncTx, mpmc};
use dashmap::DashMap;

use crate::parser::device_mux::DeviceMuxPacket;

pub struct PacketRouter {
    connections: DashMap<u16, MAsyncTx<mpmc::Array<DeviceMuxPacket>>>,
}

impl PacketRouter {
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
        }
    }

    pub fn register(&self, port: u16) -> MAsyncRx<mpmc::Array<DeviceMuxPacket>> {
        let (tx, rx) = mpmc::bounded_async(128);

        self.connections.insert(port, tx);

        rx
    }

    pub fn unregister(&self, port: u16) {
        self.connections.remove(&port);
    }

    pub async fn route(&self, packet: DeviceMuxPacket) {
        let port = packet
            .tcp_hdr
            .as_ref()
            .map(|h| h.destination_port)
            .unwrap_or(0);

        if let Some(conn) = self.connections.get(&port) {
            if conn.send(packet).await.is_err() {
                self.connections.remove(&port);
            }
        }
    }
}
