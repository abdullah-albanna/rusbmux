use bytes::{BufMut, Bytes, BytesMut};
use etherparse::TcpHeader;
use pack1::{U16BE, U32BE};
use tokio::io::AsyncReadExt;

mod builder;
pub use builder::{DeviceMuxPacketBuilder, TcpFlags};

use crate::{AsyncReading, error::ParseError};

unsafe fn to_fixed_array_unchecked<const N: usize>(slice: &[u8]) -> &[u8; N] {
    match slice.as_array() {
        Some(a) => a,
        None => unsafe { std::hint::unreachable_unchecked() },
    }
}

#[derive(Debug, Clone)]
pub struct DeviceMuxPacket {
    pub header: DeviceMuxHeader,
    pub tcp_hdr: Option<TcpHeader>,
    pub payload: DeviceMuxPayload,
}

impl DeviceMuxPacket {
    pub const HEADERS_LEN_V2: usize = DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN;

    #[inline(always)]
    #[must_use]
    pub const fn builder() -> DeviceMuxPacketBuilder {
        DeviceMuxPacketBuilder::new()
    }

    #[inline(always)]
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

    /// constructs the packet from a slice and advances it
    pub fn from_slice(s: &mut &[u8]) -> Result<Self, ParseError> {
        let header = DeviceMuxHeader::from_slice(s)?;
        let is_header_v2 = header.as_v2().is_some();
        let protocol = header.get_protocol();

        let tcp_hdr = if matches!(protocol, DeviceMuxProtocol::Tcp) && is_header_v2 {
            // if it's a tcp and it's a version 2 (no way it isn't, but just in case), then tcp is after the header v2
            let (h, rest) = TcpHeader::from_slice(s).unwrap();
            *s = rest;

            Some(h)
        } else {
            None
        };

        let tcp_hdr_len = tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        let payload_len = header.get_length() as usize - header.size() - tcp_hdr_len;

        Ok(Self {
            header,
            tcp_hdr,
            payload: DeviceMuxPayload::decode(Bytes::copy_from_slice(&s[..payload_len]), protocol),
        })
    }

    pub async fn from_reader(reader: &mut impl AsyncReading) -> Result<Self, ParseError> {
        let header = DeviceMuxHeader::from_reader(reader).await?;
        let protocol = header.get_protocol();

        let tcp_hdr = if matches!(protocol, DeviceMuxProtocol::Tcp) {
            let mut tcp_hdr_buff = [0u8; TcpHeader::MIN_LEN];

            reader.read_exact(&mut tcp_hdr_buff).await?;

            Some(TcpHeader::from_slice(&tcp_hdr_buff)?.0)
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

        reader.read_exact(&mut payload).await?;

        Ok(Self {
            header,
            tcp_hdr,
            payload: DeviceMuxPayload::decode(payload.freeze(), protocol),
        })
    }

    fn inner_encode_into(&self, buf: &mut BytesMut) {
        self.header.encode_into(buf);

        if let Some(tcp_hdr) = self.tcp_hdr.as_ref() {
            buf.extend_from_slice(tcp_hdr.to_bytes().as_slice());
        }

        self.payload.encode_into(buf);
    }

    pub fn encode_into(&self, buf: &mut BytesMut) {
        self.inner_encode_into(buf);
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        let tcp_hdr_len = self.tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        let payload_len = self.payload.len();

        let mut buf = BytesMut::with_capacity(self.header.size() + tcp_hdr_len + payload_len);

        self.inner_encode_into(&mut buf);

        buf.freeze()
    }

    pub fn get_payload_len_from_headers(&self) -> usize {
        let tcp_hdr_len = self.tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        self.header.get_length() as usize - self.header.size() - tcp_hdr_len
    }
}

#[derive(Debug, Clone)]
pub enum DeviceMuxPayload {
    Bytes(Bytes),
    Version(DeviceMuxVersion),
    Error {
        error_code: Option<u8>,
        message: Option<String>,
    },
}

impl DeviceMuxPayload {
    pub fn len(&self) -> usize {
        match self {
            Self::Bytes(b) => b.len(),
            Self::Version(_) => DeviceMuxVersion::SIZE,
            Self::Error {
                error_code,
                message,
            } => match (error_code, message) {
                (Some(_), Some(m)) => m.len() + 1,
                (Some(_), None) => 1,
                (None, _) => 0,
            },
        }
    }
    #[inline]
    pub fn decode(payload: Bytes, protocol: DeviceMuxProtocol) -> Self {
        match protocol {
            DeviceMuxProtocol::Version => Self::Version(DeviceMuxVersion::decode(&payload)),
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
            DeviceMuxProtocol::Setup | DeviceMuxProtocol::Tcp => Self::Bytes(payload),
        }
    }

    pub fn encode_into(&self, buf: &mut BytesMut) {
        match self {
            Self::Bytes(b) => buf.extend_from_slice(b),
            Self::Version(v) => buf.extend_from_slice(v.encode()),
            Self::Error {
                error_code,
                message,
            } => match (error_code, message.as_deref()) {
                (None, _) => {}
                (Some(e), None) => buf.put_u8(*e),
                (Some(e), Some(m)) => {
                    buf.put_u8(*e);
                    buf.extend_from_slice(m.as_bytes());
                }
            },
        };
    }

    pub fn encode(&self) -> Bytes {
        match self {
            Self::Bytes(b) => b.clone(),
            Self::Version(v) => Bytes::copy_from_slice(v.encode()),
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
    pub const fn as_bytes(&self) -> Option<&Bytes> {
        if let Self::Bytes(r) = self {
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
    pub fn decode(payload: &[u8]) -> Self {
        *bytemuck::from_bytes(payload)
    }

    #[inline]
    #[must_use]
    pub fn encode(&self) -> &[u8] {
        bytemuck::bytes_of(self)
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

    pub fn from_slice(s: &mut &[u8]) -> Result<Self, ParseError> {
        let protocol_buff = unsafe { to_fixed_array_unchecked::<4>(&s[..4]) };

        let protocol = DeviceMuxProtocol::new(*protocol_buff).unwrap();

        match protocol {
            DeviceMuxProtocol::Version => {
                let h = unsafe { to_fixed_array_unchecked(&s[..DeviceMuxHeaderV1::SIZE]) };
                *s = &s[DeviceMuxHeaderV1::SIZE..];

                Ok(Self::V1(*DeviceMuxHeaderV1::decode(h)))
            }
            DeviceMuxProtocol::Tcp | DeviceMuxProtocol::Setup | DeviceMuxProtocol::Control => {
                let h = unsafe { to_fixed_array_unchecked(&s[..DeviceMuxHeaderV2::SIZE]) };
                *s = &s[DeviceMuxHeaderV2::SIZE..];

                Ok(Self::V2(*DeviceMuxHeaderV2::decode(h)))
            }
        }
    }

    pub async fn from_reader(reader: &mut impl AsyncReading) -> Result<Self, ParseError> {
        // v2 and v1 share the same first bytes
        let mut header_buff = [0u8; DeviceMuxHeaderV2::SIZE];

        reader
            .read_exact(&mut header_buff[..DeviceMuxHeaderV1::SIZE])
            .await?;

        let protocol_buff: &[u8; 4] = unsafe { to_fixed_array_unchecked(&header_buff[..4]) };
        let protocol = DeviceMuxProtocol::new(*protocol_buff)?;

        match protocol {
            DeviceMuxProtocol::Version => {
                let buf =
                    unsafe { to_fixed_array_unchecked(&header_buff[..DeviceMuxHeaderV1::SIZE]) };

                Ok(Self::V1(*DeviceMuxHeaderV1::decode(buf)))
            }
            DeviceMuxProtocol::Tcp | DeviceMuxProtocol::Setup | DeviceMuxProtocol::Control => {
                reader
                    .read_exact(&mut header_buff[DeviceMuxHeaderV1::SIZE..])
                    .await?;

                Ok(Self::V2(*DeviceMuxHeaderV2::decode(&header_buff)))
            }
        }
    }

    pub fn encode_into(&self, buf: &mut BytesMut) {
        match self {
            Self::V1(v1) => buf.extend_from_slice(v1.encode()),
            Self::V2(v2) => buf.extend_from_slice(v2.encode()),
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
    pub fn decode(header: &[u8; Self::SIZE]) -> &Self {
        bytemuck::from_bytes(header)
    }

    #[inline]
    #[must_use]
    pub fn encode(&self) -> &[u8] {
        bytemuck::bytes_of(self)
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
    pub fn decode(header: &[u8; Self::SIZE]) -> &Self {
        bytemuck::from_bytes(header)
    }

    #[inline]
    #[must_use]
    pub fn encode(&self) -> &[u8] {
        bytemuck::bytes_of(self)
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

    pub fn new(v: [u8; 4]) -> Result<Self, ParseError> {
        match u32::from_be_bytes(v) {
            0 => Ok(Self::Version),
            1 => Ok(Self::Control),
            2 => Ok(Self::Setup),
            6 => Ok(Self::Tcp),
            _ => Err(ParseError::InvalidData(
                "`{value}` is not a valid device mux protocol".to_string(),
            )),
        }
    }

    #[inline]
    pub const fn from_u32_unchecked(v: u32) -> Self {
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
