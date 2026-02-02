#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MuxHeader {
    pub protocol: MuxProtocol,
    pub length: u32,
    pub magic: u32,
    pub tx_seq: u16,
    pub rx_seq: u16,
}

unsafe impl bytemuck::Zeroable for MuxHeader {}
unsafe impl bytemuck::Pod for MuxHeader {}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MuxProtocol {
    Version = 0,
    Control = 1,
    Setup = 2,
    Tcp = libc::IPPROTO_TCP as _,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MuxDevState {
    Init = 0,
    Active = 1,
    Dead = 2,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MuxConnState {
    Connecting = 0,
    Connected = 1,
    Refused = 2,
    Dying = 3,
    Dead = 4,
}
