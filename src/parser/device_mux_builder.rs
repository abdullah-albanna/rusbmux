use bytes::Bytes;
use etherparse::TcpHeader;

use crate::parser::device_mux::{
    DeviceMuxHeader, DeviceMuxHeaderV1, DeviceMuxHeaderV2, DeviceMuxPacket, DeviceMuxPayload,
    DeviceMuxProtocol, DeviceMuxVersion,
};

#[derive(Clone)]
enum BuilderPayload {
    Plist(plist::Value),
    Raw(Bytes),
    Version(DeviceMuxVersion),
}
#[derive(Clone, Default)]
pub struct DeviceMuxPacketBuilder {
    payload: Option<BuilderPayload>,
    tcp_hdr: Option<TcpHeader>,
    header: Option<(DeviceMuxProtocol, Option<(u16, u16)>)>,
}

impl DeviceMuxPacketBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn payload_version(mut self, major: u32, minor: u32) -> Self {
        self.payload = Some(BuilderPayload::Version(DeviceMuxVersion::new(
            major, minor, 0,
        )));
        self
    }

    pub fn payload_plist(mut self, value: plist::Value) -> Self {
        self.payload = Some(BuilderPayload::Plist(value));
        self
    }

    pub fn payload_raw(mut self, value: Bytes) -> Self {
        self.payload = Some(BuilderPayload::Raw(value));
        self
    }

    pub fn header_tcp(mut self, sent_seq: u16, received_seq: u16) -> Self {
        self.header = Some((DeviceMuxProtocol::Tcp, Some((sent_seq, received_seq))));
        self
    }

    pub fn header_version(mut self) -> Self {
        self.header = Some((DeviceMuxProtocol::Version, None));
        self
    }

    pub fn header_setup(mut self) -> Self {
        self.header = Some((DeviceMuxProtocol::Setup, Some((0, u16::MAX))));
        self
    }

    pub fn tcp_hdr_ack(
        mut self,
        source_port: u16,
        destination_port: u16,
        sequence_number: u32,
        acknowledgment_number: u32,
    ) -> Self {
        let mut hdr = TcpHeader::new(source_port, destination_port, sequence_number, 512);
        hdr.ack = true;
        hdr.acknowledgment_number = acknowledgment_number;
        self.tcp_hdr = Some(hdr);
        self
    }

    pub fn tcp_hdr_syn(
        mut self,
        source_port: u16,
        destination_port: u16,
        sequence_number: u32,
        acknowledgment_number: u32,
    ) -> Self {
        let mut hdr = TcpHeader::new(source_port, destination_port, sequence_number, 512);
        hdr.syn = true;
        hdr.acknowledgment_number = acknowledgment_number;
        self.tcp_hdr = Some(hdr);
        self
    }

    pub fn build(self) -> Result<DeviceMuxPacket, String> {
        let (payload, payload_len) = if let Some(p) = self.payload {
            match p {
                BuilderPayload::Plist(v) => {
                    // 1 for newline and 4 for the prefix length
                    let plist_len = plist_macro::plist_value_to_xml_bytes(&v).len() + 1 + 4;

                    (
                        DeviceMuxPayload::Plist(v, Some(plist_len as u32)),
                        plist_len,
                    )
                }
                BuilderPayload::Raw(b) => (DeviceMuxPayload::Raw(b.clone()), b.len()),
                BuilderPayload::Version(v) => {
                    (DeviceMuxPayload::Version(v), DeviceMuxVersion::SIZE)
                }
            }
        } else {
            (DeviceMuxPayload::Raw(Bytes::new()), 0)
        };

        let Some((protocol, seq)) = self.header else {
            return Err("a header is required".into());
        };

        let tcp_len = if self.tcp_hdr.is_some() {
            TcpHeader::MIN_LEN
        } else {
            0
        };

        let header = match protocol {
            DeviceMuxProtocol::Version => DeviceMuxHeader::V1(DeviceMuxHeaderV1::new(
                protocol,
                (DeviceMuxHeaderV1::SIZE + tcp_len + payload_len) as u32,
            )),
            DeviceMuxProtocol::Setup | DeviceMuxProtocol::Tcp => {
                let Some((sent_seq, received_seq)) = seq else {
                    return Err(
                        "a sent_seq and received_seq are required for the setup protocol".into(),
                    );
                };
                DeviceMuxHeader::V2(DeviceMuxHeaderV2::new(
                    protocol,
                    (DeviceMuxHeaderV2::SIZE + tcp_len + payload_len) as u32,
                    sent_seq,
                    received_seq,
                ))
            }
            _ => unimplemented!(),
        };

        Ok(DeviceMuxPacket::new(header, self.tcp_hdr, payload))
    }
}
