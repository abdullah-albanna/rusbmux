#[derive(Debug, Clone, Copy)]
pub struct UsbMuxHeader {
    pub len: u32,
    pub version: u32,
    pub message_type: u32,
    pub tag: u32,
}

unsafe impl bytemuck::Zeroable for UsbMuxHeader {}
unsafe impl bytemuck::Pod for UsbMuxHeader {}
