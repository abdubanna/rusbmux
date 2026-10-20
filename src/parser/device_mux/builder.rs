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

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct TcpFlags: u8 {
        const ACK = 1 << 0;
        const SYN = 1 << 1;
        const RST = 1 << 2;
    }
}

/// a place holder value,
///
/// it would be rewritten by the writer loop to avoid the race condition on the seq
pub const AUTO_SEQ: u16 = 0;

pub struct Empty;

#[derive(Clone)]
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

    pub fn payload_plist(self, value: plist::Value) -> UsbDevicePacketBuilder<plist::Value, H, TH> {
        UsbDevicePacketBuilder {
            payload: value,
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }

    pub fn payload_bytes(self, value: Bytes) -> UsbDevicePacketBuilder<Bytes, H, TH> {
        UsbDevicePacketBuilder {
            payload: value,
            header: self.header,
            tcp_hdr: self.tcp_hdr,
        }
    }
}

impl<P, TH> UsbDevicePacketBuilder<P, Empty, TH> {
    pub fn header_tcp(
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

    pub fn header_version(self) -> UsbDevicePacketBuilder<P, Empty, TH> {
        UsbDevicePacketBuilder {
            payload: self.payload,
            header: Empty,
            tcp_hdr: self.tcp_hdr,
        }
    }

    pub fn header_setup(self) -> UsbDevicePacketBuilder<P, (u16, u16), TH> {
        UsbDevicePacketBuilder {
            payload: self.payload,
            header: (0, u16::MAX),
            tcp_hdr: self.tcp_hdr,
        }
    }
}

impl<P, MH> UsbDevicePacketBuilder<P, MH, Empty> {
    // TODO: const ?
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

// ack
impl UsbDevicePacketBuilder<Empty, (u16, u16), TcpHeader> {
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

// full tcp packet
impl UsbDevicePacketBuilder<plist::Value, (u16, u16), TcpHeader> {
    #[must_use]
    pub fn build(self) -> UsbDevicePacket {
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

        let payload = payload.freeze();

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

// full tcp packet
impl UsbDevicePacketBuilder<Bytes, (u16, u16), TcpHeader> {
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

// version packet
impl UsbDevicePacketBuilder<UsbDevicePacketVersion, Empty, Empty> {
    #[must_use]
    pub const fn build(self) -> UsbDevicePacket {
        let payload = self.payload;

        let header = UsbDevicePacketHeader::V1(UsbDevicePacketHeaderV1::new(
            UsbDevicePacketProtocol::Version,
            (UsbDevicePacketHeaderV1::SIZE + UsbDevicePacketVersion::SIZE) as u32,
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
