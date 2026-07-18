use tokio::io::AsyncReadExt;

use crate::{AsyncReading, error::ParseError};

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

    pub async fn from_reader(reader: &mut impl AsyncReading) -> Result<Self, ParseError> {
        let header = UsbMuxHeader::from_reader(reader).await?;

        let payload_len = header
            .len
            .checked_sub(UsbMuxHeader::SIZE as _)
            .ok_or_else(|| {
                ParseError::InvalidData(format!(
                    "payload is shorter than the header, header length: {}, payload length: {}",
                    UsbMuxHeader::SIZE,
                    header.len
                ))
            })? as usize;

        // FIXME: what if the payload_len is big (manually crafted packet)
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
    pub const fn as_plist(&self) -> Option<&plist::Value> {
        match self {
            Self::Plist(p) => Some(p),
            Self::Raw(_) => None,
        }
    }

    #[must_use]
    pub const fn as_binary(&self) -> Option<&Vec<u8>> {
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

    pub fn decode(header_version: &UsbMuxVersion, payload: Vec<u8>) -> Result<Self, ParseError> {
        match header_version {
            UsbMuxVersion::Plist => {
                let plist_payload = plist::from_bytes::<plist::Value>(&payload)?;

                Ok(Self::Plist(plist_payload))
            }
            UsbMuxVersion::Binary => Ok(Self::Raw(payload)),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RawUsbMuxHeader {
    len: u32,
    version: u32,
    msg_type: u32,
    tag: u32,
}

unsafe impl bytemuck::Pod for RawUsbMuxHeader {}
unsafe impl bytemuck::Zeroable for RawUsbMuxHeader {}

#[derive(Debug, Clone, Copy)]
pub struct UsbMuxHeader {
    pub len: u32,
    pub version: UsbMuxVersion,
    pub msg_type: UsbMuxMsgType,
    pub tag: u32,
}

impl UsbMuxHeader {
    pub const SIZE: usize = size_of::<RawUsbMuxHeader>();

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let raw = RawUsbMuxHeader {
            len: self.len,
            version: self.version as u32,
            msg_type: self.msg_type as u32,
            tag: self.tag,
        };

        bytemuck::cast(raw)
    }

    pub async fn from_reader(reader: &mut impl AsyncReading) -> Result<Self, ParseError> {
        let mut buf = [0; Self::SIZE];
        reader.read_exact(&mut buf).await?;

        let raw = bytemuck::pod_read_unaligned::<RawUsbMuxHeader>(&buf);

        Ok(Self {
            len: raw.len,
            version: raw.version.try_into()?,
            msg_type: raw.msg_type.try_into()?,
            tag: raw.tag,
        })
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbMuxVersion {
    Binary = 0,
    Plist = 1,
}

impl TryFrom<u32> for UsbMuxVersion {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Binary),
            1 => Ok(Self::Plist),
            _ => Err(ParseError::InvalidData(format!(
                "`{value}` is not a valid usbmux packet version"
            ))),
        }
    }
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

impl TryFrom<u32> for UsbMuxMsgType {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Result),
            2 => Ok(Self::Connect),
            3 => Ok(Self::Listen),
            4 => Ok(Self::DeviceAdd),
            5 => Ok(Self::DeviceRemove),
            6 => Ok(Self::DevicePaired),
            8 => Ok(Self::MessagePlist),
            _ => Err(ParseError::InvalidData(format!(
                "`{value}` is not a valid usbmux message type"
            ))),
        }
    }
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

impl TryFrom<u32> for UsbMuxResult {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::BadCommand),
            2 => Ok(Self::BadDev),
            3 => Ok(Self::ConnRefused),
            6 => Ok(Self::BadVersion),
            _ => Err(ParseError::InvalidData(format!(
                "`{value}` is not a valid usbmux result"
            ))),
        }
    }
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

use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UsbMuxCommon {
    #[serde(rename = "BundleID")]
    pub bundle_id: Option<String>,

    pub client_version_string: Option<String>,
    pub conn_type: Option<u8>,

    #[serde(rename = "ProcessID")]
    pub process_id: Option<u32>,

    pub prog_name: Option<String>,

    #[serde(rename = "kLibUSBMuxVersion")]
    pub libusbmux_version: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "MessageType")]
pub enum UsbMuxRequest {
    Listen {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },

    ListDevices {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },

    ListListeners {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },
    ReadBUID {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },
    ReadPairRecord {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "PairRecordID")]
        pair_record_id: String,
    },
    SavePairRecord {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "PairRecordID")]
        pair_record_id: String,

        #[serde(rename = "PairRecordData")]
        pair_record_data: plist::Data,

        #[serde(rename = "DeviceID")]
        device_id: Option<u64>,
    },
    DeletePairRecord {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "PairRecordID")]
        pair_record_id: String,
    },
    Connect {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "DeviceID")]
        device_id: u64,

        #[serde(rename = "PortNumber", deserialize_with = "deserialize_port_number")]
        port: u16,
    },
}

fn deserialize_port_number<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let value = plist::Value::deserialize(deserializer)?;

    if let Some(ui) = value.as_unsigned_integer() {
        Ok((ui as u16).to_be())
    } else if let Some(si) = value.as_signed_integer() {
        Ok((si as u16).to_be())
    } else {
        Err(serde::de::Error::custom(
            "PortNumber is neither a signed number nor an unsigned number",
        ))
    }
}
