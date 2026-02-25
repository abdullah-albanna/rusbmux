use std::{io::Write, ops::Deref};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use etherparse::TcpHeader;
use pack1::{U16BE, U32BE};
use tokio::io::AsyncReadExt;

mod builder;
pub use builder::{DeviceMuxPacketBuilder, TcpFlags};

use crate::AsyncReading;

#[derive(Debug, Clone)]
pub struct DeviceMuxPacket {
    pub header: DeviceMuxHeader,
    pub tcp_hdr: Option<TcpHeader>,
    pub payload: DeviceMuxPayload,
}

impl DeviceMuxPacket {
    #[must_use]
    pub const fn builder() -> DeviceMuxPacketBuilder {
        DeviceMuxPacketBuilder::new()
    }

    #[must_use]
    pub const fn new(
        header: DeviceMuxHeader,
        tcp_hdr: Option<TcpHeader>,
        payload: DeviceMuxPayload,
    ) -> Self {
        Self {
            header,
            tcp_hdr,
            payload,
        }
    }

    pub async fn parse(reader: &mut impl AsyncReading) -> Self {
        let header = DeviceMuxHeader::parse(reader).await;

        let protocol = header.get_protocol();
        let tcp_hdr = if matches!(protocol, DeviceMuxProtocol::Tcp) {
            let mut tcp_hdr_buff = [0u8; TcpHeader::MIN_LEN];

            reader
                .read_exact(&mut tcp_hdr_buff)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "unable to read the {} bytes of the tcp header, error: {e}",
                        TcpHeader::MIN_LEN
                    )
                });

            Some(TcpHeader::from_slice(&tcp_hdr_buff).unwrap().0)
        } else {
            None
        };

        // the total length includes the headers, so we don't want that
        //
        // the tcp header is also counted from the total length
        let payload_len = header.get_length() as usize
            - header.size()
            - tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);

        let mut payload = BytesMut::zeroed(payload_len);

        reader
            .read_exact(&mut payload)
            .await
            .expect("unable to read the payload");

        Self {
            header,
            tcp_hdr,
            payload: DeviceMuxPayload::decode(payload.freeze(), protocol),
        }
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        let mut encoded_packet =
            BytesMut::with_capacity(DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN);

        encoded_packet.extend_from_slice(&self.header.encode());
        if let Some(tcp_hdr) = self.tcp_hdr.as_ref() {
            encoded_packet.extend_from_slice(tcp_hdr.to_bytes().as_slice());
        }
        encoded_packet.extend_from_slice(&self.payload.encode());

        encoded_packet.freeze()
    }
}

#[derive(Debug, Clone)]
pub enum DeviceMuxPayload {
    Plist(plist::Value, Option<u32>),
    Raw(Bytes),
    Version(DeviceMuxVersion),
    Error {
        error_code: Option<u8>,
        message: Option<String>,
    },
}

impl DeviceMuxPayload {
    pub fn decode(payload: Bytes, protocol: DeviceMuxProtocol) -> Self {
        match protocol {
            DeviceMuxProtocol::Version => Self::Version(DeviceMuxVersion::decode(
                payload.deref().try_into().unwrap(),
            )),
            DeviceMuxProtocol::Control => {
                if payload.len() >= 2 {
                    let error_code = payload[0];

                    let message = String::from_utf8(payload.slice(1..).to_vec())
                        .expect("unable to get the error message");

                    Self::Error {
                        error_code: Some(error_code),
                        message: Some(message),
                    }
                } else if payload.len() == 1 {
                    Self::Error {
                        error_code: Some(payload[0]),
                        message: None,
                    }
                } else {
                    Self::Error {
                        error_code: None,
                        message: None,
                    }
                }
            }
            DeviceMuxProtocol::Setup | DeviceMuxProtocol::Tcp => {
                if let Ok(p) = plist::from_bytes(&payload) {
                    Self::Plist(p, None)
                } else if payload.len() >= 4
                    && let Ok(p) = plist::from_bytes(&payload.slice(4..))
                {
                    Self::Plist(p, Some(payload.slice(0..4).get_u32()))
                } else {
                    Self::Raw(payload)
                }
            }
        }
    }

    pub fn encode(&self) -> Bytes {
        match self {
            Self::Raw(b) => b.clone(),
            Self::Plist(p, _) => {
                let mut plist_bytes_writer = BytesMut::new().writer();
                plist::to_writer_xml(&mut plist_bytes_writer, &p).unwrap();
                plist_bytes_writer.flush().unwrap();
                let plist_bytes = plist_bytes_writer.into_inner().freeze();

                // 4 for the length prefix, 1 for the \n at the end
                let mut encodede_plist = BytesMut::with_capacity(plist_bytes.len() + 4 + 1);

                // must be prefixed with length, and ends with \n
                encodede_plist.put_u32(plist_bytes.len() as u32 + 1);
                encodede_plist.extend_from_slice(&plist_bytes);
                // \n
                encodede_plist.put_u8(b'\n');
                encodede_plist.freeze()
            }
            Self::Version(v) => v.encode().to_vec().into(),
            Self::Error {
                error_code,
                message,
            } => {
                let mut encoded_error = BytesMut::new();

                if let Some(e) = error_code {
                    encoded_error.put_u8(*e);

                    if let Some(m) = message {
                        encoded_error.extend_from_slice(m.as_bytes());
                    }
                }
                encoded_error.freeze()
            }
        }
    }

    pub const fn as_plist(&self) -> Option<(&plist::Value, &Option<u32>)> {
        if let Self::Plist(p, l) = self {
            Some((p, l))
        } else {
            None
        }
    }

    pub const fn as_raw(&self) -> Option<&Bytes> {
        if let Self::Raw(r) = self {
            Some(r)
        } else {
            None
        }
    }

    pub const fn as_version(&self) -> Option<&DeviceMuxVersion> {
        if let Self::Version(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceMuxVersion {
    major: U32BE,
    minor: U32BE,
    padding: U32BE,
}

impl DeviceMuxVersion {
    pub const SIZE: usize = size_of::<Self>();

    #[must_use]
    pub const fn new(major: u32, minor: u32, padding: u32) -> Self {
        Self {
            major: U32BE::new(major),
            minor: U32BE::new(minor),
            padding: U32BE::new(padding),
        }
    }

    #[must_use]
    pub fn decode(payload: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&payload)
    }

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        bytemuck::bytes_of(self).try_into().unwrap()
    }
}

unsafe impl bytemuck::Zeroable for DeviceMuxVersion {}
unsafe impl bytemuck::Pod for DeviceMuxVersion {}

#[derive(Debug, Clone, Copy)]
pub enum DeviceMuxHeader {
    V1(DeviceMuxHeaderV1),
    V2(DeviceMuxHeaderV2),
}

impl DeviceMuxHeader {
    #[must_use]
    pub const fn as_v1(&self) -> Option<&DeviceMuxHeaderV1> {
        if let Self::V1(v1) = self {
            return Some(v1);
        }
        None
    }
    #[must_use]
    pub const fn as_v2(&self) -> Option<&DeviceMuxHeaderV2> {
        if let Self::V2(v2) = self {
            return Some(v2);
        }
        None
    }

    #[must_use]
    pub const fn size(&self) -> usize {
        match self {
            Self::V1(_) => DeviceMuxHeaderV1::SIZE,
            Self::V2(_) => DeviceMuxHeaderV2::SIZE,
        }
    }

    #[must_use]
    pub fn get_protocol(&self) -> DeviceMuxProtocol {
        match self {
            Self::V1(h) => h.protocol.get().try_into().unwrap(),
            Self::V2(h) => h.protocol.get().try_into().unwrap(),
        }
    }

    #[must_use]
    pub const fn get_length(&self) -> u32 {
        match self {
            Self::V1(h) => h.length.get(),
            Self::V2(h) => h.length.get(),
        }
    }

    pub async fn parse(reader: &mut impl AsyncReading) -> Self {
        // v2 and v1 share the same first bytes
        let mut protocol_length_buff = [0u8; DeviceMuxHeaderV1::SIZE];

        reader
            .read_exact(&mut protocol_length_buff)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "unable to read the first {} bytes of device mux header, error: {e}",
                    DeviceMuxHeaderV1::SIZE
                )
            });

        let protocol_buff: [u8; 4] = protocol_length_buff[..4].try_into().unwrap();

        let protocol = DeviceMuxProtocol::try_from(protocol_buff).unwrap();

        match protocol {
            DeviceMuxProtocol::Version => Self::V1(DeviceMuxHeaderV1::decode(protocol_length_buff)),
            DeviceMuxProtocol::Tcp | DeviceMuxProtocol::Setup | DeviceMuxProtocol::Control => {
                // both tcp and setup uses v2, so they have extra bytes
                let mut the_rest_buff = [0u8; DeviceMuxHeaderV2::SIZE - DeviceMuxHeaderV1::SIZE];

                reader
                    .read_exact(&mut the_rest_buff)
                    .await
                    .unwrap_or_else(|e| {
                        panic!(
                            "unable to read the extra {} of bytes for device mux header v2, error: {e}",
                            DeviceMuxHeaderV2::SIZE - DeviceMuxHeaderV1::SIZE
                        )
                    });

                let mut full_header_buff = [0u8; DeviceMuxHeaderV2::SIZE];

                full_header_buff[..8].copy_from_slice(&protocol_length_buff);
                full_header_buff[8..16].copy_from_slice(&the_rest_buff);

                Self::V2(DeviceMuxHeaderV2::decode(full_header_buff))
            }
        }
    }

    #[must_use]
    pub fn encode(self) -> Bytes {
        let mut encodede_header = BytesMut::with_capacity(DeviceMuxHeaderV2::SIZE);
        match self {
            Self::V1(v1) => encodede_header.extend_from_slice(&v1.encode()),
            Self::V2(v2) => encodede_header.extend_from_slice(&v2.encode()),
        }
        encodede_header.freeze()
    }
}

pub const DEVICE_MUX_HEADER_V2_MAGIC: U32BE = U32BE::new(0xfeed_face);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceMuxHeaderV2 {
    pub protocol: U32BE,
    pub length: U32BE,
    pub magic: U32BE,

    /// the nth sent packets to device
    pub send_seq: U16BE,

    /// the nth recv packets from device
    pub recv_seq: U16BE,
}

unsafe impl bytemuck::Zeroable for DeviceMuxHeaderV2 {}
unsafe impl bytemuck::Pod for DeviceMuxHeaderV2 {}

impl DeviceMuxHeaderV2 {
    pub const SIZE: usize = size_of::<Self>();

    #[must_use]
    pub const fn new(
        protocol: DeviceMuxProtocol,
        length: usize,
        send_seq: u16,
        recv_seq: u16,
    ) -> Self {
        Self {
            protocol: U32BE::new(protocol as u32),
            length: U32BE::new(length as u32),
            magic: DEVICE_MUX_HEADER_V2_MAGIC,
            send_seq: U16BE::new(send_seq),
            recv_seq: U16BE::new(recv_seq),
        }
    }

    #[must_use]
    pub fn decode(header: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&header)
    }

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        bytemuck::bytes_of(self)
            .try_into()
            .expect("the struct size doesn't change")
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceMuxHeaderV1 {
    pub protocol: U32BE,
    pub length: U32BE,
}

unsafe impl bytemuck::Zeroable for DeviceMuxHeaderV1 {}
unsafe impl bytemuck::Pod for DeviceMuxHeaderV1 {}

impl DeviceMuxHeaderV1 {
    pub const SIZE: usize = size_of::<Self>();

    #[must_use]
    pub const fn new(protocol: DeviceMuxProtocol, length: u32) -> Self {
        Self {
            protocol: U32BE::new(protocol as u32),
            length: U32BE::new(length),
        }
    }

    #[must_use]
    pub fn decode(header: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&header)
    }

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        bytemuck::bytes_of(self)
            .try_into()
            .expect("the struct size doesn't change")
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum DeviceMuxProtocol {
    Version = 0,
    Control = 1,
    Setup = 2,
    Tcp = 6,
}

impl DeviceMuxProtocol {
    pub const SIZE: usize = size_of::<Self>();

    pub const fn encode(&self) -> [u8; Self::SIZE] {
        (*self as u32).to_be_bytes()
    }
}

impl TryFrom<u32> for DeviceMuxProtocol {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Version),
            1 => Ok(Self::Control),
            2 => Ok(Self::Setup),
            6 => Ok(Self::Tcp),
            _ => Err(format!("`{value}` is not a valid device mux protocol")),
        }
    }
}

impl TryFrom<[u8; 4]> for DeviceMuxProtocol {
    type Error = String;

    /// in big endian
    fn try_from(value: [u8; 4]) -> Result<Self, Self::Error> {
        u32::from_be_bytes(value).try_into()
    }
}
