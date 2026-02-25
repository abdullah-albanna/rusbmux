use std::io::{Error as IOError, ErrorKind};

use tokio::io::AsyncReadExt;

use crate::AsyncReading;

#[derive(Debug, Clone)]
pub struct UsbMuxPacket {
    pub header: UsbMuxHeader,
    pub payload: UsbMuxPayload,
}

impl UsbMuxPacket {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let header = self.header.encode();
        let payload = self.payload.encode();

        let mut packet = Vec::with_capacity(header.len() + payload.len());

        packet.extend_from_slice(&header);
        packet.extend_from_slice(&payload);
        packet
    }

    #[must_use]
    pub fn encode_from(
        payload: Vec<u8>,
        version: UsbMuxVersion,
        msg_type: UsbMuxMsgType,
        tag: u32,
    ) -> Vec<u8> {
        Self {
            header: UsbMuxHeader {
                len: (payload.len() + UsbMuxHeader::SIZE) as u32,
                version,
                msg_type,
                tag,
            },
            payload: UsbMuxPayload::Raw(payload),
        }
        .encode()
    }

    pub async fn parse(reader: &mut impl AsyncReading) -> Result<Self, IOError> {
        let header = UsbMuxHeader::parse(reader).await?;

        let payload_len = header
            .len
            .checked_sub(UsbMuxHeader::SIZE as _)
            .ok_or_else(|| {
                IOError::new(
                    ErrorKind::InvalidData,
                    format!(
                        "payload is shorter than the header, header length: {}, payload length: {}",
                        UsbMuxHeader::SIZE,
                        header.len
                    ),
                )
            })? as usize;

        let mut payload = vec![0; payload_len];

        reader.read_exact(&mut payload).await?;

        let usbmux_payload = UsbMuxPayload::decode(&header.version, payload)?;

        Ok(Self {
            header,
            payload: usbmux_payload,
        })
    }
}

// TODO: I don't like how this looks, the `Plist` is not used, this was intended as message is in
// plist mode or binary mode
#[derive(Debug, Clone)]
pub enum UsbMuxPayload {
    Plist(plist::Value),
    Raw(Vec<u8>),
}

impl UsbMuxPayload {
    #[must_use]
    pub fn as_plist(self) -> Option<plist::Value> {
        match self {
            Self::Plist(p) => Some(p),
            Self::Raw(_) => None,
        }
    }

    #[must_use]
    pub fn as_binary(self) -> Option<Vec<u8>> {
        match self {
            Self::Raw(b) => Some(b),
            Self::Plist(_) => None,
        }
    }

    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        match self {
            Self::Plist(p) => plist_macro::plist_value_to_xml_bytes(&p),
            Self::Raw(b) => b,
        }
    }

    pub fn decode(header_version: &UsbMuxVersion, payload: Vec<u8>) -> Result<Self, IOError> {
        match header_version {
            UsbMuxVersion::Plist => {
                let plist_payload = plist::from_bytes::<plist::Value>(&payload).map_err(|e| {
                    IOError::new(
                        ErrorKind::InvalidData,
                        format!("recived invalid plist, error: {e}"),
                    )
                })?;

                Ok(Self::Plist(plist_payload))
            }
            UsbMuxVersion::Binary => Ok(Self::Raw(payload)),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsbMuxHeader {
    pub len: u32,
    pub version: UsbMuxVersion,
    pub msg_type: UsbMuxMsgType,
    pub tag: u32,
}

impl UsbMuxHeader {
    pub const SIZE: usize = size_of::<Self>();

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        bytemuck::bytes_of(self)
            .try_into()
            .expect("`UsbMuxHeader` is always 16 bytes")
    }

    pub async fn parse(reader: &mut impl AsyncReading) -> Result<Self, IOError> {
        let mut header_buf = [0; Self::SIZE];

        reader.read_exact(&mut header_buf).await?;

        bytemuck::try_from_bytes::<Self>(&header_buf)
            .copied()
            .map_err(|e| {
                IOError::new(
                    ErrorKind::InvalidData,
                    format!("unable to convert the header bytes into a UsbMuxHeader: {e}"),
                )
            })
    }
}

unsafe impl bytemuck::Zeroable for UsbMuxHeader {}
unsafe impl bytemuck::Pod for UsbMuxHeader {}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbMuxVersion {
    Binary = 0,
    Plist = 1,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbMuxResult {
    Ok = 0,
    BadCommand = 1,
    BadDev = 2,
    ConnRefused = 3,
    BadVersion = 6,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbMuxMsgType {
    Result = 1,
    Connect = 2,
    Listen = 3,
    DeviceAdd = 4,
    DeviceRemove = 5,
    DevicePaired = 6,
    MessagePlist = 8,
}

#[derive(Debug, Clone, Copy)]
pub enum PayloadMessageType {
    Listen,
    ListDevices,
    ListListeners,
    ReadBUID,
    ReadPairRecord,
    SavePairRecord,
    DeletePairRecord,
    Connect,
}

impl std::fmt::Display for PayloadMessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listen => write!(f, "Listen"),
            Self::ListDevices => write!(f, "ListDevices"),
            Self::ListListeners => write!(f, "ListListeners"),
            Self::ReadBUID => write!(f, "ReadBUID"),
            Self::ReadPairRecord => write!(f, "ReadPairRecord"),
            Self::SavePairRecord => write!(f, "SavePairRecord"),
            Self::DeletePairRecord => write!(f, "DeletePairRecord"),
            Self::Connect => write!(f, "Connect"),
        }
    }
}

impl TryFrom<&str> for PayloadMessageType {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "Listen" => Ok(Self::Listen),
            "ListDevices" => Ok(Self::ListDevices),
            "ListListeners" => Ok(Self::ListListeners),
            "ReadBUID" => Ok(Self::ReadBUID),
            "ReadPairRecord" => Ok(Self::ReadPairRecord),
            "SavePairRecord" => Ok(Self::SavePairRecord),
            "DeletePairRecord" => Ok(Self::DeletePairRecord),
            "Connect" => Ok(Self::Connect),
            _ => Err(format!("unknown payload message type: {value}")),
        }
    }
}
