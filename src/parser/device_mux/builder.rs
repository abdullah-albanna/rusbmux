use bitflags::bitflags;
use bytes::Bytes;
use etherparse::TcpHeader;

use crate::parser::device_mux::{
    DeviceMuxHeader, DeviceMuxHeaderV1, DeviceMuxHeaderV2, DeviceMuxPacket, DeviceMuxPayload,
    DeviceMuxProtocol, DeviceMuxVersion,
};

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct TcpFlags: u8 {
        const ACK = 1 << 0;
        const SYN = 1 << 1;
        const RST = 1 << 2;
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
    #[must_use]
    pub const fn new() -> Self {
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
        send_seq: u16,
        recv_seq: u16,
    ) -> DeviceMuxPacketBuilder<P, WithMuxHeader<(u16, u16)>, TH> {
        DeviceMuxPacketBuilder {
            payload: self.payload,
            header: WithMuxHeader((send_seq, recv_seq)),
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
        hdr.ack = flags.contains(TcpFlags::ACK);
        hdr.syn = flags.contains(TcpFlags::SYN);
        hdr.rst = flags.contains(TcpFlags::RST);
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
    #[must_use]
    pub const fn build(self) -> DeviceMuxPacket {
        let (send_seq, recv_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Tcp,
            DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN,
            send_seq,
            recv_seq,
        ));

        DeviceMuxPacket::new(
            header,
            Some(self.tcp_hdr),
            DeviceMuxPayload::Raw(Bytes::new()),
        )
    }
}

impl DeviceMuxPacketBuilder<WithPayload<plist::Value>, WithMuxHeader<(u16, u16)>, TcpHeader> {
    #[must_use]
    pub fn build(self) -> DeviceMuxPacket {
        let payload = self.payload.0;

        // 1 for newline and 4 for the prefix length
        // FIXME: it converts just to get the len, then the `.encode()` converts it again
        let payload_len = plist_macro::plist_value_to_xml_bytes(&payload).len() + 1 + 4;

        let (send_seq, recv_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Tcp,
            DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN + payload_len,
            send_seq,
            recv_seq,
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

        let (send_seq, recv_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Tcp,
            DeviceMuxHeaderV2::SIZE + TcpHeader::MIN_LEN + payload.len(),
            send_seq,
            recv_seq,
        ));

        DeviceMuxPacket::new(header, Some(self.tcp_hdr), DeviceMuxPayload::Raw(payload))
    }
}

impl
    DeviceMuxPacketBuilder<WithPayload<DeviceMuxVersion>, WithMuxHeader<WithNothing>, WithNothing>
{
    #[must_use]
    pub const fn build(self) -> DeviceMuxPacket {
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

        let (send_seq, recv_seq) = self.header.0;

        let header = DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
            DeviceMuxProtocol::Setup,
            DeviceMuxHeaderV2::SIZE + payload.len(),
            send_seq,
            recv_seq,
        ));

        DeviceMuxPacket::new(header, None, DeviceMuxPayload::Raw(payload))
    }
}
