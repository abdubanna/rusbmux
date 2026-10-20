use bytes::{BufMut, Bytes, BytesMut};
use etherparse::TcpHeader;
use pack1::{U16BE, U32BE};
use tokio::io::AsyncReadExt;

mod builder;
pub use builder::{AUTO_SEQ, TcpFlags, UsbDevicePacketBuilder};

use crate::{error::ParseError, usb_backend::AnyEndpointReader};

#[derive(Debug, Clone)]
pub struct UsbDevicePacket {
    pub header: UsbDevicePacketHeader,
    pub tcp_hdr: Option<TcpHeader>,
    pub payload: UsbDevicePacketPayload,
}

impl UsbDevicePacket {
    pub(crate) const HEADERS_LEN_V2: usize = UsbDevicePacketHeaderV2::SIZE + TcpHeader::MIN_LEN;

    #[inline]
    #[must_use]
    pub const fn builder() -> UsbDevicePacketBuilder {
        UsbDevicePacketBuilder::new()
    }

    #[inline]
    #[must_use]
    pub const fn new(
        header: UsbDevicePacketHeader,
        tcp_hdr: Option<TcpHeader>,
        payload: UsbDevicePacketPayload,
    ) -> Self {
        Self {
            header,
            tcp_hdr,
            payload,
        }
    }

    /// constructs the packet from a slice and advances it
    pub fn from_slice(s: &mut &[u8]) -> Result<Self, ParseError> {
        let orig_len = s.len();
        let header = UsbDevicePacketHeader::from_slice(s)?;
        let is_header_v2 = header.as_v2().is_some();
        let protocol = header.get_protocol();

        let consumed = orig_len - s.len();

        let tcp_hdr = if matches!(protocol, UsbDevicePacketProtocol::Tcp) && is_header_v2 {
            let (h, rest) = TcpHeader::from_slice(s).map_err(|err| {
                ParseError::InvalidData(format!("failed to parse TCP header from slice: {err}"))
            })?;
            *s = rest;

            Some(h)
        } else {
            None
        };

        let tcp_hdr_len = tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        let total_header_len = header.size() + tcp_hdr_len;
        let total_length = header.get_length() as usize;

        if total_length < total_header_len {
            return Err(ParseError::InvalidData(format!(
                "packet length {} is less than header size {}",
                total_length, total_header_len
            )));
        }

        let payload_len = total_length - total_header_len;

        if payload_len > s.len() {
            return Err(ParseError::InvalidData(format!(
                "packet length {} exceeds remaining slice length {}",
                total_length,
                s.len() + consumed
            )));
        }

        let payload = Bytes::copy_from_slice(&s[..payload_len]);
        *s = &s[payload_len..];

        Ok(Self {
            header,
            tcp_hdr,
            payload: UsbDevicePacketPayload::decode(payload, protocol),
        })
    }

    pub async fn from_reader(reader: &mut AnyEndpointReader) -> Result<Self, ParseError> {
        let header = UsbDevicePacketHeader::from_reader(reader).await?;
        let protocol = header.get_protocol();

        let tcp_hdr = if matches!(protocol, UsbDevicePacketProtocol::Tcp) {
            // NOTE: does the device send tcp options?
            let mut tcp_hdr_buff = [0u8; TcpHeader::MIN_LEN];

            reader.read_exact(&mut tcp_hdr_buff).await?;

            Some(TcpHeader::from_slice(&tcp_hdr_buff)?.0)
        } else {
            None
        };

        let tcp_hdr_len = tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        let total_header_len = header.size() + tcp_hdr_len;
        let total_length = header.get_length() as usize;

        if total_length < total_header_len {
            return Err(ParseError::InvalidData(format!(
                "packet length {} is less than header size {}",
                total_length, total_header_len
            )));
        }

        let payload_len = total_length - total_header_len;

        // if payload_len > MAX_PACKET_PAYLOAD_SIZE {
        //     return Err(ParseError::InvalidData(format!(
        //         "packet payload length {payload_len} exceeds the maximum {MAX_PACKET_PAYLOAD_SIZE}"
        //     )));
        // }

        let mut payload = BytesMut::with_capacity(payload_len);

        // SAFETY: `read_exact` below overwrites every one of those bytes before `payload`
        // is read
        unsafe {
            payload.set_len(payload_len);
        }

        reader.read_exact(&mut payload).await?;

        Ok(Self {
            header,
            tcp_hdr,
            payload: UsbDevicePacketPayload::decode(payload.freeze(), protocol),
        })
    }

    fn inner_encode_into(&self, buf: &mut BytesMut) {
        self.header.encode_into(buf);

        if let Some(tcp_hdr) = self.tcp_hdr.as_ref() {
            buf.extend_from_slice(tcp_hdr.to_bytes().as_slice());
        }

        self.payload.encode_into(buf);
    }

    pub fn encode_into(&self, buf: &mut BytesMut) {
        self.inner_encode_into(buf);
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        let tcp_hdr_len = self.tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        let payload_len = self.payload.len();

        let mut buf = BytesMut::with_capacity(self.header.size() + tcp_hdr_len + payload_len);

        self.inner_encode_into(&mut buf);

        buf.freeze()
    }

    pub fn get_payload_len_from_headers(&self) -> usize {
        let tcp_hdr_len = self.tcp_hdr.as_ref().map_or(0, TcpHeader::header_len);
        self.header.get_length() as usize - self.header.size() - tcp_hdr_len
    }
}

#[derive(Debug, Clone)]
pub enum UsbDevicePacketPayload {
    Bytes(Bytes),
    Version(UsbDevicePacketVersion),
    Error {
        error_code: Option<u8>,
        message: Option<String>,
    },
}

impl UsbDevicePacketPayload {
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub const fn len(&self) -> usize {
        match self {
            Self::Bytes(b) => b.len(),
            Self::Version(_) => UsbDevicePacketVersion::SIZE,
            Self::Error {
                error_code,
                message,
            } => match (error_code, message) {
                (Some(_), Some(m)) => m.len() + 1,
                (Some(_), None) => 1,
                (None, _) => 0,
            },
        }
    }
    #[inline]
    pub fn decode(payload: Bytes, protocol: UsbDevicePacketProtocol) -> Self {
        match protocol {
            UsbDevicePacketProtocol::Version => {
                Self::Version(UsbDevicePacketVersion::decode(&payload))
            }
            UsbDevicePacketProtocol::Control => match payload.len() {
                0 => Self::Error {
                    error_code: None,
                    message: None,
                },
                1 => Self::Error {
                    error_code: Some(payload[0]),
                    message: None,
                },
                _ => {
                    let error_code = payload[0];
                    let message = String::from_utf8_lossy(&payload[1..]).to_string();
                    Self::Error {
                        error_code: Some(error_code),
                        message: Some(message),
                    }
                }
            },
            UsbDevicePacketProtocol::Setup | UsbDevicePacketProtocol::Tcp => Self::Bytes(payload),
        }
    }

    pub fn encode_into(&self, buf: &mut BytesMut) {
        match self {
            Self::Bytes(b) => buf.extend_from_slice(b),
            Self::Version(v) => buf.extend_from_slice(v.encode()),
            Self::Error {
                error_code,
                message,
            } => match (error_code, message.as_deref()) {
                (None, _) => {}
                (Some(e), None) => buf.put_u8(*e),
                (Some(e), Some(m)) => {
                    buf.put_u8(*e);
                    buf.extend_from_slice(m.as_bytes());
                }
            },
        }
    }

    pub fn encode(self) -> Bytes {
        match self {
            Self::Bytes(b) => b,
            Self::Version(v) => Bytes::copy_from_slice(v.encode()),
            Self::Error {
                error_code,
                message,
            } => match (error_code, message.as_deref()) {
                (None, _) => Bytes::new(),
                (Some(e), None) => Bytes::copy_from_slice(&[e]),
                (Some(e), Some(m)) => {
                    let mut encoded_error = BytesMut::with_capacity(1 + m.len());

                    encoded_error.put_u8(e);
                    encoded_error.extend_from_slice(m.as_bytes());
                    encoded_error.freeze()
                }
            },
        }
    }

    #[inline]
    pub const fn as_bytes(&self) -> Option<&Bytes> {
        if let Self::Bytes(r) = self {
            Some(r)
        } else {
            None
        }
    }

    #[inline]
    pub const fn as_version(&self) -> Option<&UsbDevicePacketVersion> {
        if let Self::Version(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsbDevicePacketVersion {
    major: U32BE,
    minor: U32BE,
    padding: U32BE,
}

impl UsbDevicePacketVersion {
    pub const SIZE: usize = size_of::<Self>();

    #[inline]
    #[must_use]
    pub const fn new(major: u32, minor: u32, padding: u32) -> Self {
        Self {
            major: U32BE::new(major),
            minor: U32BE::new(minor),
            padding: U32BE::new(padding),
        }
    }

    #[inline]
    #[must_use]
    pub fn decode(payload: &[u8]) -> Self {
        *bytemuck::from_bytes(payload)
    }

    #[inline]
    #[must_use]
    pub fn encode(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

unsafe impl bytemuck::Zeroable for UsbDevicePacketVersion {}
unsafe impl bytemuck::Pod for UsbDevicePacketVersion {}

#[derive(Debug, Clone, Copy)]
pub enum UsbDevicePacketHeader {
    V1(UsbDevicePacketHeaderV1),
    V2(UsbDevicePacketHeaderV2),
}

impl UsbDevicePacketHeader {
    #[inline]
    #[must_use]
    pub const fn as_v1(&self) -> Option<&UsbDevicePacketHeaderV1> {
        if let Self::V1(v1) = self {
            return Some(v1);
        }
        None
    }
    #[inline]
    #[must_use]
    pub const fn as_v2(&self) -> Option<&UsbDevicePacketHeaderV2> {
        if let Self::V2(v2) = self {
            return Some(v2);
        }
        None
    }

    #[inline]
    #[must_use]
    pub const fn size(&self) -> usize {
        match self {
            Self::V1(_) => UsbDevicePacketHeaderV1::SIZE,
            Self::V2(_) => UsbDevicePacketHeaderV2::SIZE,
        }
    }

    #[must_use]
    pub const fn get_protocol(&self) -> UsbDevicePacketProtocol {
        let raw = match self {
            Self::V1(h) => h.protocol.get(),
            Self::V2(h) => h.protocol.get(),
        };
        // SAFETY: value came from a validated decode path
        UsbDevicePacketProtocol::from_u32_unchecked(raw)
    }

    #[must_use]
    pub const fn get_length(&self) -> u32 {
        match self {
            Self::V1(h) => h.length.get(),
            Self::V2(h) => h.length.get(),
        }
    }

    pub fn from_slice(s: &mut &[u8]) -> Result<Self, ParseError> {
        if s.len() < 4 {
            return Err(ParseError::InvalidData(
                "slice too short for header".to_string(),
            ));
        }

        let protocol_buff: &[u8; 4] = &s[..4]
            .try_into()
            .map_err(|_| ParseError::InvalidData("failed to read protocol bytes".to_string()))?;

        let protocol = UsbDevicePacketProtocol::new(*protocol_buff)?;

        match protocol {
            UsbDevicePacketProtocol::Version => {
                let h = unsafe {
                    &s[..UsbDevicePacketHeaderV1::SIZE]
                        .try_into()
                        .unwrap_unchecked()
                };
                *s = &s[UsbDevicePacketHeaderV1::SIZE..];

                Ok(Self::V1(*UsbDevicePacketHeaderV1::decode(h)))
            }
            UsbDevicePacketProtocol::Tcp
            | UsbDevicePacketProtocol::Setup
            | UsbDevicePacketProtocol::Control => {
                let h = unsafe {
                    &s[..UsbDevicePacketHeaderV2::SIZE]
                        .try_into()
                        .unwrap_unchecked()
                };
                *s = &s[UsbDevicePacketHeaderV2::SIZE..];

                Ok(Self::V2(*UsbDevicePacketHeaderV2::decode(h)))
            }
        }
    }

    pub async fn from_reader(reader: &mut AnyEndpointReader) -> Result<Self, ParseError> {
        // v2 and v1 share the same first bytes
        let mut header_buff = [0u8; UsbDevicePacketHeaderV2::SIZE];

        reader
            .read_exact(&mut header_buff[..UsbDevicePacketHeaderV1::SIZE])
            .await?;

        let protocol_buff: &[u8; 4] = &header_buff[..4].try_into().map_err(|_| {
            ParseError::InvalidData("failed to read protocol bytes from reader".to_string())
        })?;
        let protocol = UsbDevicePacketProtocol::new(*protocol_buff)?;

        match protocol {
            UsbDevicePacketProtocol::Version => {
                let buf = unsafe {
                    &header_buff[..UsbDevicePacketHeaderV1::SIZE]
                        .try_into()
                        .unwrap_unchecked()
                };

                Ok(Self::V1(*UsbDevicePacketHeaderV1::decode(buf)))
            }
            UsbDevicePacketProtocol::Tcp
            | UsbDevicePacketProtocol::Setup
            | UsbDevicePacketProtocol::Control => {
                reader
                    .read_exact(&mut header_buff[UsbDevicePacketHeaderV1::SIZE..])
                    .await?;

                Ok(Self::V2(*UsbDevicePacketHeaderV2::decode(&header_buff)))
            }
        }
    }

    pub fn encode_into(&self, buf: &mut BytesMut) {
        match self {
            Self::V1(v1) => buf.extend_from_slice(v1.encode()),
            Self::V2(v2) => buf.extend_from_slice(v2.encode()),
        }
    }

    #[inline]
    #[must_use]
    pub fn encode(self) -> Bytes {
        let mut encodede_header = BytesMut::with_capacity(UsbDevicePacketHeaderV2::SIZE);
        self.encode_into(&mut encodede_header);
        encodede_header.freeze()
    }
}

pub const DEVICE_MUX_HEADER_V2_MAGIC: U32BE = U32BE::new(0xfeed_face);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsbDevicePacketHeaderV2 {
    pub protocol: U32BE,
    pub length: U32BE,
    pub magic: U32BE,

    /// the nth sent packets to device
    pub send_seq: U16BE,

    /// the nth recv packets from device
    pub recv_seq: U16BE,
}

unsafe impl bytemuck::Zeroable for UsbDevicePacketHeaderV2 {}
unsafe impl bytemuck::Pod for UsbDevicePacketHeaderV2 {}

impl UsbDevicePacketHeaderV2 {
    pub const SIZE: usize = size_of::<Self>();

    #[inline]
    #[must_use]
    pub const fn new(
        protocol: UsbDevicePacketProtocol,
        length: usize,
        send_seq: u16,
        recv_seq: u16,
    ) -> Self {
        Self {
            protocol: U32BE::new(protocol as u32),
            length: U32BE::new(length as u32),
            magic: DEVICE_MUX_HEADER_V2_MAGIC,
            send_seq: U16BE::new(send_seq),
            recv_seq: U16BE::new(recv_seq),
        }
    }

    #[inline]
    #[must_use]
    pub fn decode(header: &[u8; Self::SIZE]) -> &Self {
        bytemuck::from_bytes(header)
    }

    #[inline]
    #[must_use]
    pub fn encode(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsbDevicePacketHeaderV1 {
    pub protocol: U32BE,
    pub length: U32BE,
}

unsafe impl bytemuck::Zeroable for UsbDevicePacketHeaderV1 {}
unsafe impl bytemuck::Pod for UsbDevicePacketHeaderV1 {}

impl UsbDevicePacketHeaderV1 {
    pub const SIZE: usize = size_of::<Self>();

    #[inline]
    #[must_use]
    pub const fn new(protocol: UsbDevicePacketProtocol, length: u32) -> Self {
        Self {
            protocol: U32BE::new(protocol as u32),
            length: U32BE::new(length),
        }
    }

    #[inline]
    #[must_use]
    pub fn decode(header: &[u8; Self::SIZE]) -> &Self {
        bytemuck::from_bytes(header)
    }

    #[inline]
    #[must_use]
    pub fn encode(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbDevicePacketProtocol {
    Version = 0,
    Control = 1,
    Setup = 2,
    Tcp = 6,
}

impl UsbDevicePacketProtocol {
    pub const SIZE: usize = size_of::<Self>();

    #[must_use]
    pub const fn encode(&self) -> [u8; Self::SIZE] {
        (*self as u32).to_be_bytes()
    }

    pub fn new(v: [u8; 4]) -> Result<Self, ParseError> {
        match u32::from_be_bytes(v) {
            0 => Ok(Self::Version),
            1 => Ok(Self::Control),
            2 => Ok(Self::Setup),
            6 => Ok(Self::Tcp),
            value => Err(ParseError::InvalidData(format!(
                "`{value}` is not a valid device mux protocol"
            ))),
        }
    }

    #[must_use]
    pub const fn from_u32_unchecked(v: u32) -> Self {
        unsafe { std::mem::transmute(v) }
    }
}

impl TryFrom<u32> for UsbDevicePacketProtocol {
    type Error = String;

    #[inline]
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Version),
            1 => Ok(Self::Control),
            2 => Ok(Self::Setup),
            6 => Ok(Self::Tcp),
            _ => Err(format!("`{value}` is not a valid device mux protocol")),
        }
    }
}

impl TryFrom<[u8; 4]> for UsbDevicePacketProtocol {
    type Error = String;

    /// in big endian
    #[inline]
    fn try_from(value: [u8; 4]) -> Result<Self, Self::Error> {
        u32::from_be_bytes(value).try_into()
    }
}
