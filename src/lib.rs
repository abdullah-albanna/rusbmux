#[cfg(target_os = "windows")]
compile_error!("windows is currently not supported due to how usb access is restricted");

use tokio::io::{AsyncRead, AsyncWrite};

pub mod daemon;
pub mod device;
pub mod error;
pub mod handler;
pub mod packet_router;
pub mod parser;
pub mod usb;
pub mod utils;
pub mod watcher;

pub trait ReadWrite: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> ReadWrite for T {}

pub trait AsyncReading: AsyncRead + Unpin + Send + Sync {}
impl<T: AsyncRead + Unpin + Send + Sync> AsyncReading for T {}

pub trait AsyncWriting: AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncWrite + Unpin + Send + Sync> AsyncWriting for T {}
