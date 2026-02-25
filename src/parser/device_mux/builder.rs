use bytes::Bytes;
use etherparse::TcpHeader;

use crate::parser::device_mux::{
    DeviceMuxHeader, DeviceMuxHeaderV1, DeviceMuxHeaderV2, DeviceMuxPacket, DeviceMuxPayload,
    DeviceMuxProtocol, DeviceMuxVersion,
};

#[derive(Clone, Copy)]
pub struct TcpFlags(u8);

impl TcpFlags {
    pub const ACK: Self = Self(1 << 1);
    pub const RST: Self = Self(1 << 2);
    pub const SYN: Self = Self(1 << 3);
}

impl std::ops::BitOr for TcpFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}
impl std::ops::BitAnd for TcpFlags {
    type Output = u8;
    fn bitand(self, rhs: Self) -> Self::Output {
        self.0 & rhs.0
    }
}

pub struct WithPayload<P>(P);
pub struct WithMuxHeader<MH>(MH);

pub struct WithNothing;

#[derive(Clone)]
pub struct DeviceMuxPacketBuilder<P = WithNothing, MH = WithNothing, TH = WithNothing> {
    payload: P,
    header: MH,
    tcp_hdr: TH,
}

impl Default for DeviceMuxPacketBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceMuxPacketBuilder {
    pub fn new() -> Self {
        Self {
            payload: WithNothing,
            header: WithNothing,
            tcp_hdr: WithNothing,
        }
    }
}

impl<MH, TH> DeviceMuxPacketBuilder<WithNothing, MH, TH> {
    pub fn payload_version(
        self,
        major: u32,
        minor: u32,
    ) -> DeviceMuxPacketBuilder<WithPayload<DeviceMuxVersion>, MH, TH> {
        DeviceMuxPacketBuilder {
            payload: WithPayload(DeviceMuxVersion::new(major, minor, 0)),
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }

    pub fn payload_plist(
        self,
        value: plist::Value,
    ) -> DeviceMuxPacketBuilder<WithPayload<plist::Value>, MH, TH> {
        DeviceMuxPacketBuilder {
            payload: WithPayload(value),
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }

    pub fn payload_raw(self, value: Bytes) -> DeviceMuxPacketBuilder<WithPayload<Bytes>, MH, TH> {
        DeviceMuxPacketBuilder {
            payload: WithPayload(value),
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }
}

impl<P, TH> DeviceMuxPacketBuilder<P, WithNothing, TH> {
    pub fn header_tcp(
        self,
        sent_seq: u16,
        received_seq: u16,
    ) -> DeviceMuxPacketBuilder<P, WithMuxHeader<(u16, u16)>, TH> {
        DeviceMuxPacketBuilder {
            payload: self.payload,
            header: WithMuxHeader((sent_seq, received_seq)),
            tcp_hdr: self.tcp_hdr,
        }
    }

    pub fn header_version(self) -> DeviceMuxPacketBuilder<P, WithMuxHeader<WithNothing>, TH> {
        DeviceMuxPacketBuilder {
            payload: self.payload,
            header: WithMuxHeader(WithNothing),
            tcp_hdr: self.tcp_hdr,
        }
    }

    pub fn header_setup(self) -> DeviceMuxPacketBuilder<P, WithMuxHeader<(u16, u16)>, TH> {
        DeviceMuxPacketBuilder {
            payload: self.payload,
            header: WithMuxHeader((0, u16::MAX)),
            tcp_hdr: self.tcp_hdr,
        }
    }
}

impl<P, MH> DeviceMuxPacketBuilder<P, MH, WithNothing> {
    pub fn tcp_header(
        self,
        source_port: u16,
        destination_port: u16,
        sequence_number: u32,
        acknowledgment_number: u32,
        flags: TcpFlags,
    ) -> DeviceMuxPacketBuilder<P, MH, TcpHeader> {
        let mut hdr = TcpHeader::new(source_port, destination_port, sequence_number, 512);
        hdr.ack = (flags & TcpFlags::ACK) != 0;
        hdr.syn = (flags & TcpFlags::SYN) != 0;
        hdr.rst = (flags & TcpFlags::RST) != 0;
        hdr.acknowledgment_number = acknowledgment_number;

        DeviceMuxPacketBuilder {
            payload: self.payload,
            header: self.header,
            tcp_hdr: hdr,
        }
    }
}

// ack
impl DeviceMuxPacketBuilder<WithNothing, WithMuxHeader<(u16, u16)>, TcpHeader> {
    pub fn build(self) -> DeviceMuxPacket {
        let (sent_seq, received_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Tcp,
            (DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN) as u32,
            sent_seq,
            received_seq,
        ));

        DeviceMuxPacket::new(
            header,
            Some(self.tcp_hdr),
            DeviceMuxPayload::Raw(Bytes::new()),
        )
    }
}

impl DeviceMuxPacketBuilder<WithPayload<plist::Value>, WithMuxHeader<(u16, u16)>, TcpHeader> {
    pub fn build(self) -> DeviceMuxPacket {
        let payload = self.payload.0;

        // 1 for newline and 4 for the prefix length
        // FIXME: it converts just to get the len, then the `.encode()` converts it again
        let payload_len = plist_macro::plist_value_to_xml_bytes(&payload).len() + 1 + 4;

        let (sent_seq, received_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Tcp,
            (DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN + payload_len) as u32,
            sent_seq,
            received_seq,
        ));

        DeviceMuxPacket::new(
            header,
            Some(self.tcp_hdr),
            DeviceMuxPayload::Plist(payload, Some(payload_len as u32)),
        )
    }
}

impl DeviceMuxPacketBuilder<WithPayload<Bytes>, WithMuxHeader<(u16, u16)>, TcpHeader> {
    pub fn build(self) -> DeviceMuxPacket {
        let payload = self.payload.0;

        let (sent_seq, received_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Tcp,
            (DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN + payload.len()) as u32,
            sent_seq,
            received_seq,
        ));

        DeviceMuxPacket::new(header, Some(self.tcp_hdr), DeviceMuxPayload::Raw(payload))
    }
}

impl
    DeviceMuxPacketBuilder<WithPayload<DeviceMuxVersion>, WithMuxHeader<WithNothing>, WithNothing>
{
    pub fn build(self) -> DeviceMuxPacket {
        let payload = self.payload.0;

        let header = DeviceMuxHeader::V1(DeviceMuxHeaderV1::new(
            DeviceMuxProtocol::Version,
            (DeviceMuxHeaderV1::SIZE + DeviceMuxVersion::SIZE) as u32,
        ));

        DeviceMuxPacket::new(header, None, DeviceMuxPayload::Version(payload))
    }
}

impl DeviceMuxPacketBuilder<WithPayload<Bytes>, WithMuxHeader<(u16, u16)>, WithNothing> {
    pub fn build(self) -> DeviceMuxPacket {
        let payload = self.payload.0;

        let (sent_seq, received_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Setup,
            (DeviceMuxHeaderV2::SIZE + payload.len()) as u32,
            sent_seq,
            received_seq,
        ));

        DeviceMuxPacket::new(header, None, DeviceMuxPayload::Raw(payload))
    }
}
