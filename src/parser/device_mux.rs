use etherparse::TcpHeader;
use pack1::{U16BE, U32BE};
use tokio::io::AsyncReadExt;

use crate::AsyncReading;

#[derive(Debug, Clone)]
pub struct DeviceMuxPacket {
    pub header: DeviceMuxHeader,
    pub tcp_hdr: Option<TcpHeader>,
    pub payload: DeviceMuxPayload,
}

impl DeviceMuxPacket {
    pub async fn parse(reader: &mut impl AsyncReading) -> Self {
        let header = DeviceMuxHeader::parse(reader).await;

        let protocol = header.get_protocol();
        let tcp_hdr = if let DeviceMuxProtocol::Tcp = protocol {
            let mut tcp_hdr_buff = [0u8; 20];

            reader
                .read_exact(&mut tcp_hdr_buff)
                .await
                .expect("unable to read the 20 bytes of the tcp header");

            Some(TcpHeader::from_slice(&tcp_hdr_buff).unwrap().0)
        } else {
            None
        };

        // the total length includes the headers, so we don't want that
        //
        // the tcp header is also counted from the total length
        let payload_len = header.get_length() as usize
            - header.size()
            - tcp_hdr.as_ref().map_or(0, |h| h.header_len());

        let mut payload = vec![0u8; payload_len];

        reader
            .read_exact(&mut payload)
            .await
            .expect("unable to read the payload");

        Self {
            header,
            tcp_hdr,
            payload: DeviceMuxPayload::decode(payload, protocol),
        }
    }
}

#[derive(Debug, Clone)]
pub enum DeviceMuxPayload {
    Plist(plist::Value),
    Raw(Vec<u8>),
    Version(DeviceMuxVersion),
    Error {
        error_code: Option<u8>,
        message: Option<String>,
    },
}

impl DeviceMuxPayload {
    pub fn decode(payload: Vec<u8>, protocol: DeviceMuxProtocol) -> Self {
        match protocol {
            DeviceMuxProtocol::Version => Self::Version(DeviceMuxVersion::decode(&payload)),
            DeviceMuxProtocol::Control => {
                if payload.len() >= 2 {
                    let error_code = payload[0];

                    // FIXME: feels like I can do better
                    let message = String::from_utf8(payload[1..].to_vec())
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
                    Self::Plist(p)
                } else if payload.len() >= 4
                    && let Ok(p) = plist::from_bytes(&payload[4..])
                {
                    Self::Plist(p)
                } else {
                    Self::Raw(payload)
                }
            }
        }
    }

    pub fn as_plist(&self) -> Option<&plist::Value> {
        if let Self::Plist(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_raw(&self) -> Option<&Vec<u8>> {
        if let Self::Raw(r) = self {
            Some(r)
        } else {
            None
        }
    }

    pub fn as_version(&self) -> Option<&DeviceMuxVersion> {
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

    pub fn decode(payload: &[u8]) -> Self {
        *bytemuck::from_bytes(payload)
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
    pub fn size(&self) -> usize {
        match self {
            Self::V1(_) => DeviceMuxHeaderV1::SIZE,
            Self::V2(_) => DeviceMuxHeaderV2::SIZE,
        }
    }

    pub fn get_protocol(&self) -> DeviceMuxProtocol {
        match self {
            Self::V1(h) => h.protocol.get().try_into().unwrap(),
            Self::V2(h) => h.protocol.get().try_into().unwrap(),
        }
    }

    pub fn get_length(&self) -> u32 {
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
}

pub const DEVICE_MUX_HEADER_V2_MAGIC: U32BE = U32BE::new(0xfeedface);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceMuxHeaderV2 {
    pub protocol: U32BE,
    pub length: U32BE,
    pub magic: U32BE,

    /// the nth sent packets to device
    pub sender_seq: U16BE,

    /// the nth recevied packets from device
    pub recevier_seq: U16BE,
}

unsafe impl bytemuck::Zeroable for DeviceMuxHeaderV2 {}
unsafe impl bytemuck::Pod for DeviceMuxHeaderV2 {}

impl DeviceMuxHeaderV2 {
    pub const SIZE: usize = size_of::<Self>();

    pub fn new(
        protocol: DeviceMuxProtocol,
        length: u32,
        sender_seq: u16,
        recevier_seq: u16,
    ) -> Self {
        Self {
            protocol: U32BE::new(protocol as u32),
            length: U32BE::new(length),
            magic: DEVICE_MUX_HEADER_V2_MAGIC,
            sender_seq: U16BE::new(sender_seq),
            recevier_seq: U16BE::new(recevier_seq),
        }
    }

    pub fn decode(header: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&header)
    }

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

    pub fn new(protocol: DeviceMuxProtocol, length: u32) -> Self {
        Self {
            protocol: U32BE::new(protocol as u32),
            length: U32BE::new(length),
        }
    }

    pub fn decode(header: [u8; Self::SIZE]) -> Self {
        *bytemuck::from_bytes(&header)
    }

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

    pub fn encode(self) -> [u8; Self::SIZE] {
        (self as u32).to_be_bytes()
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
