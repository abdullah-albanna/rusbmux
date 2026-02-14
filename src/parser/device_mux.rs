use etherparse::TcpHeader;
use pack1::{U16BE, U32BE};
use tokio::io::AsyncReadExt;

use crate::AsyncReading;

#[derive(Debug, Clone)]
pub struct DeviceMuxPacket {
    pub header: DeviceMuxHeader,
    pub tcp_hdr: Option<TcpHeader>,
    pub payload: Vec<u8>,
}

impl DeviceMuxPacket {
    pub async fn parse(reader: &mut impl AsyncReading) -> Self {
        // both v1 and v2 have the same first 8 bytes
        let mut protocol_length_buff = [0u8; DeviceMuxHeaderV1::SIZE];

        reader
            .read_exact(&mut protocol_length_buff)
            .await
            .expect("unable to read the first 8 bytes of device mux header");

        let protocol_buff: [u8; 4] = protocol_length_buff[..4].try_into().unwrap();
        let total_length_buff: [u8; 4] = protocol_length_buff[4..8].try_into().unwrap();
        let total_length = u32::from_be_bytes(total_length_buff);

        let protocol = DeviceMuxProtocol::decode(U32BE::new(u32::from_be_bytes(protocol_buff)));

        let header = match protocol {
            DeviceMuxProtocol::Version => {
                DeviceMuxHeader::V1(DeviceMuxHeaderV1::decode(protocol_length_buff))
            }
            DeviceMuxProtocol::Tcp | DeviceMuxProtocol::Setup => {
                let mut the_rest_buff = [0u8; DeviceMuxHeaderV2::SIZE - DeviceMuxHeaderV1::SIZE];

                reader
                    .read_exact(&mut the_rest_buff)
                    .await
                    .expect("unable to read the rest of the 8 bytes of device mux header v2");

                let mut full_header_buff = [0u8; DeviceMuxHeaderV2::SIZE];

                full_header_buff[..8].copy_from_slice(&protocol_length_buff);
                full_header_buff[8..16].copy_from_slice(&the_rest_buff);

                DeviceMuxHeader::V2(DeviceMuxHeaderV2::decode(full_header_buff))
            }
        };

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
        let payload_len =
            total_length as usize - header.size() - tcp_hdr.as_ref().map_or(0, |h| h.header_len());

        let mut payload = vec![0u8; payload_len];

        reader
            .read_exact(&mut payload)
            .await
            .expect("unable to read the payload");

        Self {
            header,
            tcp_hdr,
            payload,
        }
    }
}

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
pub enum DeviceMuxProtocol {
    Version = 0,
    Setup = 2,
    Tcp = 6,
}

impl DeviceMuxProtocol {
    pub const SIZE: usize = size_of::<Self>();

    pub fn decode(protocol: U32BE) -> Self {
        protocol.get().try_into().unwrap()
    }
}

impl TryFrom<u32> for DeviceMuxProtocol {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Version),
            2 => Ok(Self::Setup),
            6 => Ok(Self::Tcp),
            // _ => Err(format!("`{value}` is not a valid device mux protocol")),
            _ => Ok(Self::Version),
        }
    }
}
