use std::ops::Deref;

use bytes::{BufMut, Bytes, BytesMut};
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
    pub const HEADERS_LEN_V2: usize = DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN;

    #[inline]
    #[must_use]
    pub const fn builder() -> DeviceMuxPacketBuilder {
        DeviceMuxPacketBuilder::new()
    }

    #[inline]
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

    pub async fn from_reader(reader: &mut impl AsyncReading) -> Self {
        let header = DeviceMuxHeader::from_reader(reader).await;
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
        let tcp_hdr_len = tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        let payload_len = header.get_length() as usize - header.size() - tcp_hdr_len;

        // SAFETY: we immediately overwrite every byte via read_exact
        let mut payload = BytesMut::with_capacity(payload_len);
        unsafe { payload.set_len(payload_len) };

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

    #[inline]
    #[must_use]
    pub fn encode(&self) -> Bytes {
        let encoded_payload = self.payload.encode();
        let tcp_hdr_len = self.tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        let mut encoded_packet =
            BytesMut::with_capacity(self.header.size() + tcp_hdr_len + encoded_payload.len());

        match &self.header {
            DeviceMuxHeader::V1(v1) => encoded_packet.extend_from_slice(&v1.encode()),
            DeviceMuxHeader::V2(v2) => encoded_packet.extend_from_slice(&v2.encode()),
        }

        if let Some(tcp_hdr) = self.tcp_hdr.as_ref() {
            encoded_packet.extend_from_slice(tcp_hdr.to_bytes().as_slice());
        }

        encoded_packet.extend_from_slice(&encoded_payload);

        encoded_packet.freeze()
    }

    #[inline]
    pub fn get_payload_len(&self) -> usize {
        let tcp_hdr_len = self.tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        self.header.get_length() as usize - self.header.size() - tcp_hdr_len
    }
}

#[derive(Debug, Clone)]
pub enum DeviceMuxPayload {
    Raw(Bytes),
    Version(DeviceMuxVersion),
    Error {
        error_code: Option<u8>,
        message: Option<String>,
    },
}

impl DeviceMuxPayload {
    #[inline]
    pub fn decode(payload: Bytes, protocol: DeviceMuxProtocol) -> Self {
        match protocol {
            DeviceMuxProtocol::Version => Self::Version(DeviceMuxVersion::decode(
                payload.deref().try_into().unwrap(),
            )),
            DeviceMuxProtocol::Control => match payload.len() {
                0 => Self::Error {
                    error_code: None,
                    message: None,
                },
                1 => Self::Error {
                    error_code: Some(payload[0]),
                    message: None,
                },
                _ => {
                    let error_code = payload[0];
                    let message = std::str::from_utf8(&payload[1..])
                        .expect("unable to get the error message")
                        .to_owned();
                    Self::Error {
                        error_code: Some(error_code),
                        message: Some(message),
                    }
                }
            },
            DeviceMuxProtocol::Setup | DeviceMuxProtocol::Tcp => Self::Raw(payload),
        }
    }

    #[inline]
    pub fn encode(&self) -> Bytes {
        match self {
            Self::Raw(b) => b.clone(),
            Self::Version(v) => Bytes::copy_from_slice(&v.encode()),
            Self::Error {
                error_code,
                message,
            } => match (error_code, message.as_deref()) {
                (None, _) => Bytes::new(),
                (Some(e), None) => Bytes::copy_from_slice(&[*e]),
                (Some(e), Some(m)) => {
                    let mut encoded_error = BytesMut::with_capacity(1 + m.len());

                    encoded_error.put_u8(*e);
                    encoded_error.extend_from_slice(m.as_bytes());
                    encoded_error.freeze()
                }
            },
        }
    }

    #[inline]
    pub const fn as_raw(&self) -> Option<&Bytes> {
        if let Self::Raw(r) = self {
            Some(r)
        } else {
            None
        }
    }

    #[inline]
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

    #[inline]
    #[must_use]
    pub const fn new(major: u32, minor: u32, padding: u32) -> Self {
        Self {
            major: U32BE::new(major),
            minor: U32BE::new(minor),
            padding: U32BE::new(padding),
        }
    }

    #[inline]
    #[must_use]
    pub fn decode(payload: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&payload)
    }

    #[inline]
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
    #[inline]
    #[must_use]
    pub const fn as_v1(&self) -> Option<&DeviceMuxHeaderV1> {
        if let Self::V1(v1) = self {
            return Some(v1);
        }
        None
    }
    #[inline]
    #[must_use]
    pub const fn as_v2(&self) -> Option<&DeviceMuxHeaderV2> {
        if let Self::V2(v2) = self {
            return Some(v2);
        }
        None
    }

    #[inline]
    #[must_use]
    pub const fn size(&self) -> usize {
        match self {
            Self::V1(_) => DeviceMuxHeaderV1::SIZE,
            Self::V2(_) => DeviceMuxHeaderV2::SIZE,
        }
    }

    #[inline]
    #[must_use]
    pub fn get_protocol(&self) -> DeviceMuxProtocol {
        let raw = match self {
            Self::V1(h) => h.protocol.get(),
            Self::V2(h) => h.protocol.get(),
        };
        // SAFETY: value came from a validated decode path
        DeviceMuxProtocol::from_u32_unchecked(raw)
    }

    #[inline]
    #[must_use]
    pub const fn get_length(&self) -> u32 {
        match self {
            Self::V1(h) => h.length.get(),
            Self::V2(h) => h.length.get(),
        }
    }

    pub async fn from_reader(reader: &mut impl AsyncReading) -> Self {
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
                // read the remaining bytes directly into a full-size buffer
                let mut full_header_buff = [0u8; DeviceMuxHeaderV2::SIZE];
                full_header_buff[..DeviceMuxHeaderV1::SIZE].copy_from_slice(&protocol_length_buff);

                reader
                    .read_exact(&mut full_header_buff[DeviceMuxHeaderV1::SIZE..])
                    .await
                    .unwrap_or_else(|e| {
                        panic!(
                            "unable to read the extra {} of bytes for device mux header v2, error: {e}",
                            DeviceMuxHeaderV2::SIZE - DeviceMuxHeaderV1::SIZE
                        )
                    });

                Self::V2(DeviceMuxHeaderV2::decode(full_header_buff))
            }
        }
    }

    #[inline]
    pub fn encode_into(&self, buf: &mut BytesMut) {
        match self {
            Self::V1(v1) => buf.extend_from_slice(&v1.encode()),
            Self::V2(v2) => buf.extend_from_slice(&v2.encode()),
        }
    }

    #[inline]
    #[must_use]
    pub fn encode(self) -> Bytes {
        let mut encodede_header = BytesMut::with_capacity(DeviceMuxHeaderV2::SIZE);
        self.encode_into(&mut encodede_header);
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

    #[inline]
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

    #[inline]
    #[must_use]
    pub fn decode(header: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&header)
    }

    #[inline]
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

    #[inline]
    #[must_use]
    pub const fn new(protocol: DeviceMuxProtocol, length: u32) -> Self {
        Self {
            protocol: U32BE::new(protocol as u32),
            length: U32BE::new(length),
        }
    }

    #[inline]
    #[must_use]
    pub fn decode(header: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&header)
    }

    #[inline]
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

    #[inline]
    pub const fn encode(&self) -> [u8; Self::SIZE] {
        (*self as u32).to_be_bytes()
    }

    #[inline]
    pub fn from_u32_unchecked(v: u32) -> Self {
        debug_assert!(
            matches!(v, 0 | 1 | 2 | 6),
            "invalid device mux protocol: {v}"
        );
        unsafe { std::mem::transmute(v) }
    }
}

impl TryFrom<u32> for DeviceMuxProtocol {
    type Error = String;

    #[inline]
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
    #[inline]
    fn try_from(value: [u8; 4]) -> Result<Self, Self::Error> {
        u32::from_be_bytes(value).try_into()
    }
}
