use bitflags::bitflags;
use bytes::{BufMut, Bytes, BytesMut};
use etherparse::TcpHeader;

use crate::{
    conn::UsbDeviceConn,
    parser::device_mux::{
        UsbDevicePacket, UsbDevicePacketHeader, UsbDevicePacketHeaderV1, UsbDevicePacketHeaderV2,
        UsbDevicePacketPayload, UsbDevicePacketProtocol, UsbDevicePacketVersion,
    },
};

/// stores the kind of packets that are being sent the writer loop of the device
///
/// it's generic to being either v1 or v2, the writer loop decided what it should be depending on
/// the device version
#[derive(Debug, Clone)]
pub enum UsbDevicePacketBuilderKind {
    /// v1 or v2 tcp flag payload (ACK, RST, etc)
    TcpFlag(UsbDevicePacketBuilder<Empty, Empty, TcpHeader>),

    /// v1 or v2 full payload with unencoded plist payload
    FullPlist(UsbDevicePacketBuilder<plist::Value, Empty, TcpHeader>),

    /// v1 or v2 full payload with encoded bytes payload
    FullBytes(UsbDevicePacketBuilder<Bytes, Empty, TcpHeader>),
}

impl UsbDevicePacketBuilderKind {
    #[inline]
    pub fn build_v2(self, send_seq: u16, recv_seq: u16) -> UsbDevicePacket {
        match self {
            Self::TcpFlag(pb) => pb.header_v2(send_seq, recv_seq).build(),
            Self::FullPlist(pb) => pb.header_v2(send_seq, recv_seq).build(),
            Self::FullBytes(pb) => pb.header_v2(send_seq, recv_seq).build(),
        }
    }

    #[inline]
    pub fn build_v1(self) -> UsbDevicePacket {
        match self {
            Self::TcpFlag(pb) => pb.build(),
            Self::FullPlist(pb) => pb.build(),
            Self::FullBytes(pb) => pb.build(),
        }
    }

    #[inline]
    pub fn payload_len(&self) -> usize {
        match self {
            Self::TcpFlag(_) => 0,
            Self::FullBytes(pb) => pb.payload.len(),
            // TODO: expensive, maybe we can convert the plist::Value into a Bytes packet
            Self::FullPlist(pb) => pb.encode_payload().len(),
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct TcpFlags: u8 {
        const ACK = 1 << 0;
        const SYN = 1 << 1;
        const RST = 1 << 2;
    }
}

#[derive(Debug, Clone)]
pub struct Empty;

#[derive(Debug, Clone)]
pub struct UsbDevicePacketBuilder<P = Empty, H = Empty, TH = Empty> {
    payload: P,
    header: H,
    tcp_hdr: TH,
}

impl Default for UsbDevicePacketBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl UsbDevicePacketBuilder {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            payload: Empty,
            header: Empty,
            tcp_hdr: Empty,
        }
    }
}

impl<H, TH> UsbDevicePacketBuilder<Empty, H, TH> {
    #[inline]
    pub fn payload_version(
        self,
        major: u32,
        minor: u32,
    ) -> UsbDevicePacketBuilder<UsbDevicePacketVersion, H, TH> {
        UsbDevicePacketBuilder {
            payload: UsbDevicePacketVersion::new(major, minor, 0),
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }

    #[inline]
    pub fn payload_plist(self, value: plist::Value) -> UsbDevicePacketBuilder<plist::Value, H, TH> {
        UsbDevicePacketBuilder {
            payload: value,
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }

    #[inline]
    pub fn payload_bytes(self, value: Bytes) -> UsbDevicePacketBuilder<Bytes, H, TH> {
        UsbDevicePacketBuilder {
            payload: value,
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }
}

impl<P, TH> UsbDevicePacketBuilder<P, Empty, TH> {
    #[inline]
    pub fn header_v2(
        self,
        send_seq: u16,
        recv_seq: u16,
    ) -> UsbDevicePacketBuilder<P, (u16, u16), TH> {
        UsbDevicePacketBuilder {
            payload: self.payload,
            header: (send_seq, recv_seq),
            tcp_hdr: self.tcp_hdr,
        }
    }

    #[inline]
    pub fn header_version(self) -> UsbDevicePacketBuilder<P, Empty, TH> {
        UsbDevicePacketBuilder {
            payload: self.payload,
            header: Empty,
            tcp_hdr: self.tcp_hdr,
        }
    }

    #[inline]
    pub fn header_setup(self) -> UsbDevicePacketBuilder<P, (u16, u16), TH> {
        UsbDevicePacketBuilder {
            payload: self.payload,
            header: (0, u16::MAX),
            tcp_hdr: self.tcp_hdr,
        }
    }
}

impl<P, MH> UsbDevicePacketBuilder<P, MH, Empty> {
    pub fn tcp_header(
        self,
        source_port: u16,
        destination_port: u16,
        sequence_number: u32,
        acknowledgment_number: u32,
        flags: TcpFlags,
    ) -> UsbDevicePacketBuilder<P, MH, TcpHeader> {
        let mut hdr = TcpHeader::new(
            source_port,
            destination_port,
            sequence_number,
            // TODO: is this suppose to change?
            UsbDeviceConn::WINDOW_SIZE,
        );
        hdr.ack = flags.contains(TcpFlags::ACK);
        hdr.syn = flags.contains(TcpFlags::SYN);
        hdr.rst = flags.contains(TcpFlags::RST);
        hdr.acknowledgment_number = acknowledgment_number;

        UsbDevicePacketBuilder {
            payload: self.payload,
            header: self.header,
            tcp_hdr: hdr,
        }
    }
}

impl<MH, TH> UsbDevicePacketBuilder<plist::Value, MH, TH> {
    #[inline]
    fn encode_payload(&self) -> Bytes {
        // an empty plist is sized at 181 (with the length prefix and \n)
        // with one empty key-value is 220
        let mut payload_writer = BytesMut::with_capacity(250).writer();

        // length prefix place holder
        payload_writer.get_mut().put_u32(0);

        self.payload.to_writer_xml(&mut payload_writer).unwrap();

        payload_writer.get_mut().put_u8(b'\n');

        let mut payload = payload_writer.into_inner();
        let payload_len = (payload.len() - 4) as u32;

        payload[..4].copy_from_slice(&payload_len.to_be_bytes());

        payload.freeze()
    }
}

impl UsbDevicePacketBuilder<Empty, (u16, u16), TcpHeader> {
    /// tcp flag packet v2 (ACK, RST, etc)
    #[must_use]
    pub const fn build(self) -> UsbDevicePacket {
        let (send_seq, recv_seq) = self.header;

        let header = UsbDevicePacketHeader::V2(UsbDevicePacketHeaderV2::new(
            UsbDevicePacketProtocol::Tcp,
            UsbDevicePacketHeaderV2::SIZE + TcpHeader::MIN_LEN,
            send_seq,
            recv_seq,
        ));

        UsbDevicePacket::new(
            header,
            Some(self.tcp_hdr),
            UsbDevicePacketPayload::Bytes(Bytes::new()),
        )
    }
}

impl UsbDevicePacketBuilder<Empty, Empty, TcpHeader> {
    /// tcp flag packet v1 (ACK, RST, etc)
    #[must_use]
    pub const fn build(self) -> UsbDevicePacket {
        let header = UsbDevicePacketHeader::V1(UsbDevicePacketHeaderV1::new(
            UsbDevicePacketProtocol::Tcp,
            UsbDevicePacketHeaderV1::SIZE + TcpHeader::MIN_LEN,
        ));

        UsbDevicePacket::new(
            header,
            Some(self.tcp_hdr),
            UsbDevicePacketPayload::Bytes(Bytes::new()),
        )
    }
}

impl UsbDevicePacketBuilder<plist::Value, (u16, u16), TcpHeader> {
    /// full tcp v2 packet with unencoded plist payload
    #[must_use]
    pub fn build(self) -> UsbDevicePacket {
        let (send_seq, recv_seq) = self.header;

        let payload = self.encode_payload();

        let header = UsbDevicePacketHeader::V2(UsbDevicePacketHeaderV2::new(
            UsbDevicePacketProtocol::Tcp,
            UsbDevicePacketHeaderV2::SIZE + TcpHeader::MIN_LEN + payload.len(),
            send_seq,
            recv_seq,
        ));

        UsbDevicePacket::new(
            header,
            Some(self.tcp_hdr),
            UsbDevicePacketPayload::Bytes(payload),
        )
    }
}

impl UsbDevicePacketBuilder<plist::Value, Empty, TcpHeader> {
    /// full tcp v1 packet with unencoded plist payload
    #[must_use]
    pub fn build(self) -> UsbDevicePacket {
        let payload = self.encode_payload();

        let header = UsbDevicePacketHeader::V1(UsbDevicePacketHeaderV1::new(
            UsbDevicePacketProtocol::Tcp,
            UsbDevicePacketHeaderV1::SIZE + TcpHeader::MIN_LEN + payload.len(),
        ));

        UsbDevicePacket::new(
            header,
            Some(self.tcp_hdr),
            UsbDevicePacketPayload::Bytes(payload),
        )
    }
}

impl UsbDevicePacketBuilder<Bytes, (u16, u16), TcpHeader> {
    /// full tcp v2 packet with encoded payload
    pub fn build(self) -> UsbDevicePacket {
        let payload = self.payload;

        let (send_seq, recv_seq) = self.header;

        let header = UsbDevicePacketHeader::V2(UsbDevicePacketHeaderV2::new(
            UsbDevicePacketProtocol::Tcp,
            UsbDevicePacketHeaderV2::SIZE + TcpHeader::MIN_LEN + payload.len(),
            send_seq,
            recv_seq,
        ));

        UsbDevicePacket::new(
            header,
            Some(self.tcp_hdr),
            UsbDevicePacketPayload::Bytes(payload),
        )
    }
}

impl UsbDevicePacketBuilder<Bytes, Empty, TcpHeader> {
    /// full tcp v1 packet with encoded payload
    pub fn build(self) -> UsbDevicePacket {
        let payload = self.payload;

        let header = UsbDevicePacketHeader::V1(UsbDevicePacketHeaderV1::new(
            UsbDevicePacketProtocol::Tcp,
            UsbDevicePacketHeaderV1::SIZE + TcpHeader::MIN_LEN + payload.len(),
        ));

        UsbDevicePacket::new(
            header,
            Some(self.tcp_hdr),
            UsbDevicePacketPayload::Bytes(payload),
        )
    }
}

// version packet
impl UsbDevicePacketBuilder<UsbDevicePacketVersion, Empty, Empty> {
    #[must_use]
    pub const fn build(self) -> UsbDevicePacket {
        let payload = self.payload;

        let header = UsbDevicePacketHeader::V1(UsbDevicePacketHeaderV1::new(
            UsbDevicePacketProtocol::Version,
            UsbDevicePacketHeaderV1::SIZE + UsbDevicePacketVersion::SIZE,
        ));

        UsbDevicePacket::new(header, None, UsbDevicePacketPayload::Version(payload))
    }
}

// setup packet
impl UsbDevicePacketBuilder<Bytes, (u16, u16), Empty> {
    pub fn build(self) -> UsbDevicePacket {
        let payload = self.payload;

        let (send_seq, recv_seq) = self.header;

        let header = UsbDevicePacketHeader::V2(UsbDevicePacketHeaderV2::new(
            UsbDevicePacketProtocol::Setup,
            UsbDevicePacketHeaderV2::SIZE + payload.len(),
            send_seq,
            recv_seq,
        ));

        UsbDevicePacket::new(header, None, UsbDevicePacketPayload::Bytes(payload))
    }
}
