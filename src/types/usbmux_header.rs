#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsbMuxHeader {
    pub len: u32,
    pub version: UsbMuxVersion,
    pub msg_type: UsbMuxMsgType,
    pub tag: u32,
}

unsafe impl bytemuck::Zeroable for UsbMuxHeader {}
unsafe impl bytemuck::Pod for UsbMuxHeader {}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbMuxVersion {
    Binary = 0,
    Plist = 1,
}

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
