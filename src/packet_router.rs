use std::sync::Arc;

use arc_swap::ArcSwapOption;
use crossfire::{MAsyncRx, MAsyncTx, mpmc};

use crate::parser::device_mux::DeviceMuxPacket;

pub struct PacketRouter {
    connections: Box<[ArcSwapOption<MAsyncTx<mpmc::Array<DeviceMuxPacket>>>]>,
}

impl Default for PacketRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketRouter {
    pub fn new() -> Self {
        let mut vec = Vec::with_capacity(65536);
        for _ in 0..65536 {
            vec.push(ArcSwapOption::const_empty());
        }

        Self {
            connections: vec.into_boxed_slice(),
        }
    }

    pub fn register(&self, port: u16) -> MAsyncRx<mpmc::Array<DeviceMuxPacket>> {
        let (tx, rx) = mpmc::bounded_async(128);

        self.connections[port as usize].store(Some(Arc::new(tx)));

        rx
    }

    pub fn unregister(&self, port: u16) {
        self.connections[port as usize].store(None);
    }

    #[inline(always)]
    pub async fn route(&self, packet: DeviceMuxPacket) {
        let port = packet
            .tcp_hdr
            .as_ref()
            .map(|h| h.destination_port)
            .unwrap_or(0);

        if let Some(conn) = self.connections[port as usize].load_full()
            && conn.send(packet).await.is_err()
        {
            self.unregister(port);
        }
    }
}
