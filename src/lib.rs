#[derive(Debug, Clone, Copy)]
struct RawUsbMuxHeader {
    len: u32,
    version: u32,
    msg_type: u32,
    tag: u32,
}

unsafe impl bytemuck::Zeroable for RawUsbMuxHeader {}
unsafe impl bytemuck::Pod for RawUsbMuxHeader {}

#[derive(Debug, Clone, Copy)]
pub struct UsbMuxHeader {
    pub len: u32,
    pub version: UsbMuxVersion,
    pub msg_type: UsbMuxMsgType,
    pub tag: u32,
}

impl UsbMuxHeader {
    pub fn parse(buff: [u8; 16]) -> UsbMuxHeader {
        let raw_header: &RawUsbMuxHeader = bytemuck::from_bytes(&buff);

        UsbMuxHeader {
            len: raw_header.len,
            tag: raw_header.tag,
            version: raw_header.version.into(),
            msg_type: raw_header.msg_type.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum UsbMuxVersion {
    Binary = 0,
    Plist = 1,
}

impl From<u32> for UsbMuxVersion {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::Binary,
            1 => Self::Plist,
            _ => unreachable!("there's no such {value} version"),
        }
    }
}

pub enum UsbMuxResult {
    Ok = 0,
    BadCommand = 1,
    BadDev = 2,
    ConnRefused = 3,
    BadVersion = 6,
}

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

impl From<u32> for UsbMuxMsgType {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::Result,
            2 => Self::Connect,
            3 => Self::Listen,
            4 => Self::DeviceAdd,
            5 => Self::DeviceRemove,
            6 => Self::DevicePaired,
            8 => Self::MessagePlist,
            _ => unreachable!("idk, we'll see how it goes"),
        }
    }
}
