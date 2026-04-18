use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{Mutex, watch},
};
use tracing::debug;

use crate::error::RusbmuxError;

pub struct NetworkDeviceConn {
    // TODO: maybe we can not use a mutex
    pub stream: Mutex<TcpStream>,

    pub device_id: u64,

    pub destination_port: u16,

    pub shutdown_rx: watch::Receiver<()>,
}

impl NetworkDeviceConn {
    pub async fn new(
        socket: SocketAddr,
        device_id: u64,
        shutdown_rx: watch::Receiver<()>,
    ) -> Result<Self, RusbmuxError> {
        dbg!(socket);
        let stream = Mutex::new(TcpStream::connect(socket).await?);

        Ok(Self {
            stream,
            device_id,
            destination_port: socket.port(),
            shutdown_rx,
        })
    }

    pub async fn write(&self, value: Bytes) -> Result<(), RusbmuxError> {
        debug!(?value);
        Ok(self.stream.lock().await.write_all(&value).await?)
    }

    pub async fn read(&self) -> Result<Bytes, RusbmuxError> {
        let mut buf = BytesMut::new();

        let n = self.stream.lock().await.read_buf(&mut buf).await?;

        if n == 0 {
            return Err(RusbmuxError::IO(std::io::Error::new(
                std::io::ErrorKind::NetworkUnreachable,
                "Stream returned 0",
            )));
        }

        let packet = buf.split().freeze();

        debug!(?packet);

        Ok(packet)
    }

    pub async fn wait_shutdown(&self) {
        self.shutdown_rx.clone().changed().await.unwrap();
    }
}
