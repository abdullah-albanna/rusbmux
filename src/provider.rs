use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::{Buf, BytesMut};
use idevice::{Idevice, IdeviceError, pairing_file::PairingFile, provider::IdeviceProvider};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    conn::UsbDeviceConn, device::usb::UsbDevice, error::RusbmuxError,
    parser::device_mux::UsbDevicePacket,
};

/// a provider that exposes rusbmux's direct USB connection as an `idevice` provider
///
/// allows the `idevice` crate to connect to devices over USB without going
/// through a socket
#[derive(Debug)]
pub struct UsbMuxProvider {
    device: Arc<UsbDevice>,
    pairing_file: Option<PairingFile>,
    label: String,
}

impl UsbMuxProvider {
    pub fn new(device: Arc<UsbDevice>, pairing_file: Option<PairingFile>, label: String) -> Self {
        Self {
            device,
            pairing_file,
            label,
        }
    }

    pub fn set_pairing_file(&mut self, pairing_file: PairingFile) {
        self.pairing_file = Some(pairing_file);
    }
}

impl IdeviceProvider for UsbMuxProvider {
    fn label(&self) -> &str {
        &self.label
    }

    fn get_pairing_file(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PairingFile, IdeviceError>> + Send>> {
        let pairing_file = self.pairing_file.clone();
        Box::pin(async move {
            if let Some(pairing_file) = pairing_file {
                Ok(pairing_file)
            } else {
                Err(IdeviceError::NotFound)
            }
        })
    }

    fn connect(
        &self,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Idevice, IdeviceError>> + Send>> {
        let label = self.label.clone();
        let device = Arc::clone(&self.device);

        let udid = self
            .device
            .info
            .serial_number()
            .unwrap_or_default()
            .to_string();

        Box::pin(async move {
            let conn = device.connect(port).await.map_err(|e| {
                IdeviceError::UnexpectedResponse(format!(
                    "failed to connect to port {port} on {udid}: {e}"
                ))
            })?;

            let stream = UsbMuxStream {
                device,
                conn,
                read_buf: BytesMut::new(),
                write_buf: BytesMut::new(),
                inflight_len: 0,
                pending_read_fut: None,
                pending_write_fut: None,
            };

            let mut idevice = Idevice::new(Box::new(stream), label);
            idevice.set_udid(udid);

            Ok(idevice)
        })
    }
}

/// bridges an rusbmux `UsbDeviceConn` to idevice's `ReadWrite` trait
///
/// mirrors the bidirectional pattern in daemon mode
///
/// incoming USB packets are buffered until consumed by the reader, while
/// outgoing writes are split according to the connection's current window size
struct UsbMuxStream {
    device: Arc<UsbDevice>,
    conn: Arc<UsbDeviceConn>,
    read_buf: BytesMut,
    write_buf: BytesMut,
    inflight_len: usize,
    pending_read_fut: Option<
        Pin<Box<dyn std::future::Future<Output = Result<UsbDevicePacket, RusbmuxError>> + Send>>,
    >,
    pending_write_fut:
        Option<Pin<Box<dyn std::future::Future<Output = Result<(), RusbmuxError>> + Send>>>,
}

unsafe impl Sync for UsbMuxStream {}

impl std::fmt::Debug for UsbMuxStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbMuxStream")
            .field("device", &self.device)
            .field("conn", &self.conn)
            .finish_non_exhaustive()
    }
}

impl AsyncRead for UsbMuxStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // 1 - read the buffered if it has something
        //
        // 2 - if not, and there's a pending read future, await it
        //   2.1 - once done, save it and go to step 1
        //
        // 3 - if not, then do a new pending read and go to step 1
        loop {
            if !self.read_buf.is_empty() {
                let n = self.read_buf.len().min(buf.remaining());
                buf.put_slice(&self.read_buf[..n]);
                self.read_buf.advance(n);
                if buf.remaining() != 0 && !self.read_buf.is_empty() {
                    continue;
                }
                return Poll::Ready(Ok(()));
            } else if let Some(ref mut fut) = self.pending_read_fut {
                match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(packet)) => {
                        let payload = packet.payload.encode();
                        self.read_buf = BytesMut::from(payload);
                        self.pending_read_fut = None;
                        continue;
                    }
                    Poll::Ready(Err(e)) => {
                        self.pending_read_fut = None;
                        return Poll::Ready(Err(io_error(e)));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            } else {
                let conn = Arc::clone(&self.conn);
                self.pending_read_fut = Some(Box::pin(async move { conn.recv().await }));
                continue;
            }
        }
    }
}

impl AsyncWrite for UsbMuxStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // 1 - if there's a pending write, await it
        //
        // 2 - if we can't write right now, try to read (to increase the window size)
        //   2.1 - if there's an already pending read, await it
        //     2.1.1 - once done, save it to the read buffer, and go to step 1
        //   2.2 - if there's not an already pending read, do a new one and go to step 2.1
        //
        // 3 - if the write buffer has something, take min(sendable.len(), buffer.len())
        //     and do a new pending write, and go to step 1
        //
        // 4 - else, save the given buffer to our write buffer, take min(sendable.len(), buffer.len())
        //     and do a new pending write, and go to step 1
        loop {
            if let Some(ref mut fut) = self.pending_write_fut {
                match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(())) => {
                        self.pending_write_fut = None;
                        return Poll::Ready(Ok(self.inflight_len));
                    }
                    Poll::Ready(Err(e)) => {
                        self.pending_write_fut = None;
                        return Poll::Ready(Err(io_error(e)));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            } else if self.conn.get_sendable_bytes() == 0 {
                loop {
                    if let Some(ref mut fut) = self.pending_read_fut {
                        match fut.as_mut().poll(cx) {
                            Poll::Ready(Ok(packet)) => {
                                let payload = packet.payload.encode();
                                if self.read_buf.is_empty() {
                                    self.read_buf = payload.into();
                                } else {
                                    self.read_buf.extend_from_slice(&payload);
                                }
                                self.pending_read_fut = None;
                                break;
                            }
                            Poll::Ready(Err(e)) => {
                                self.pending_read_fut = None;
                                return Poll::Ready(Err(io_error(e)));
                            }
                            Poll::Pending => return Poll::Pending,
                        }
                    } else {
                        let conn = Arc::clone(&self.conn);
                        self.pending_read_fut = Some(Box::pin(async move { conn.recv().await }));
                    }
                }
            } else if !self.write_buf.is_empty() {
                // FIXME: redundant code
                let sendable = self.conn.get_sendable_bytes();

                let n = self.write_buf.len().min(sendable);

                let chunk = self.write_buf.split_to(n);

                self.inflight_len = n;

                let conn = Arc::clone(&self.conn);

                self.pending_write_fut =
                    Some(Box::pin(
                        async move { conn.send_bytes(chunk.freeze()).await },
                    ));
            } else {
                self.write_buf.extend_from_slice(buf);

                let sendable = self.conn.get_sendable_bytes();
                let n = self.write_buf.len().min(sendable);

                let chunk = self.write_buf.split_to(n);
                self.inflight_len = n;

                let conn = Arc::clone(&self.conn);

                self.pending_write_fut =
                    Some(Box::pin(
                        async move { conn.send_bytes(chunk.freeze()).await },
                    ));
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if let Some(ref mut fut) = self.pending_write_fut {
            match fut.as_mut().poll(cx) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(e)) => {
                    self.pending_write_fut = None;
                    Poll::Ready(Err(io_error(e)))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }
}

fn io_error(e: RusbmuxError) -> std::io::Error {
    std::io::Error::other(e)
}
