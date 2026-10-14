use serde::{Deserialize, Deserializer};
use std::net::IpAddr;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::ParseError;

#[derive(Debug, Clone)]
pub struct UsbMuxPacket {
    pub header: UsbMuxHeader,
    pub payload: UsbMuxPayload,
}

impl UsbMuxPacket {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let header = self.header.encode();
        let payload = self.payload.encode();

        let mut packet = Vec::with_capacity(header.len() + payload.len());

        packet.extend_from_slice(&header);
        packet.extend_from_slice(&payload);
        packet
    }

    #[must_use]
    pub fn encode_from(
        payload: Vec<u8>,
        version: UsbMuxVersion,
        msg_type: UsbMuxMsgType,
        tag: u32,
    ) -> Vec<u8> {
        Self {
            header: UsbMuxHeader {
                len: (payload.len() + UsbMuxHeader::SIZE) as u32,
                version,
                msg_type,
                tag,
            },
            payload: UsbMuxPayload::Raw(payload),
        }
        .encode()
    }

    pub async fn from_reader(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self, ParseError> {
        let header = UsbMuxHeader::from_reader(reader).await?;

        let payload_len = header
            .len
            .checked_sub(UsbMuxHeader::SIZE as _)
            .ok_or_else(|| {
                ParseError::InvalidData(format!(
                    "payload is shorter than the header, header length: {}, payload length: {}",
                    UsbMuxHeader::SIZE,
                    header.len
                ))
            })? as usize;

        // FIXME: what if the payload_len is big (manually crafted packet)
        let mut payload = vec![0; payload_len];

        reader.read_exact(&mut payload).await?;

        let usbmux_payload = UsbMuxPayload::decode(&header.version, payload)?;

        Ok(Self {
            header,
            payload: usbmux_payload,
        })
    }
}

// TODO: I don't like how this looks, the `Plist` is not used, this was intended as message is in
// plist mode or binary mode
#[derive(Debug, Clone)]
pub enum UsbMuxPayload {
    Plist(plist::Value),
    Raw(Vec<u8>),
}

impl UsbMuxPayload {
    #[must_use]
    pub const fn as_plist(&self) -> Option<&plist::Value> {
        match self {
            Self::Plist(p) => Some(p),
            Self::Raw(_) => None,
        }
    }

    #[must_use]
    pub const fn as_binary(&self) -> Option<&Vec<u8>> {
        match self {
            Self::Raw(b) => Some(b),
            Self::Plist(_) => None,
        }
    }

    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        match self {
            Self::Plist(p) => plist_macro::plist_value_to_xml_bytes(&p),
            Self::Raw(b) => b,
        }
    }

    pub fn decode(header_version: &UsbMuxVersion, payload: Vec<u8>) -> Result<Self, ParseError> {
        match header_version {
            UsbMuxVersion::Plist => {
                let plist_payload = plist::from_bytes::<plist::Value>(&payload)?;

                Ok(Self::Plist(plist_payload))
            }
            UsbMuxVersion::Binary => Ok(Self::Raw(payload)),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RawUsbMuxHeader {
    len: u32,
    version: u32,
    msg_type: u32,
    tag: u32,
}

unsafe impl bytemuck::Pod for RawUsbMuxHeader {}
unsafe impl bytemuck::Zeroable for RawUsbMuxHeader {}

#[derive(Debug, Clone, Copy)]
pub struct UsbMuxHeader {
    pub len: u32,
    pub version: UsbMuxVersion,
    pub msg_type: UsbMuxMsgType,
    pub tag: u32,
}

impl UsbMuxHeader {
    pub const SIZE: usize = size_of::<RawUsbMuxHeader>();

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let raw = RawUsbMuxHeader {
            len: self.len,
            version: self.version as u32,
            msg_type: self.msg_type as u32,
            tag: self.tag,
        };

        bytemuck::cast(raw)
    }

    pub async fn from_reader(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self, ParseError> {
        let mut buf = [0; Self::SIZE];
        reader.read_exact(&mut buf).await?;

        let raw = bytemuck::pod_read_unaligned::<RawUsbMuxHeader>(&buf);

        Ok(Self {
            len: raw.len,
            version: raw.version.try_into()?,
            msg_type: raw.msg_type.try_into()?,
            tag: raw.tag,
        })
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbMuxVersion {
    Binary = 0,
    Plist = 1,
}

impl TryFrom<u32> for UsbMuxVersion {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Binary),
            1 => Ok(Self::Plist),
            _ => Err(ParseError::InvalidData(format!(
                "`{value}` is not a valid usbmux packet version"
            ))),
        }
    }
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

impl TryFrom<u32> for UsbMuxMsgType {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Result),
            2 => Ok(Self::Connect),
            3 => Ok(Self::Listen),
            4 => Ok(Self::DeviceAdd),
            5 => Ok(Self::DeviceRemove),
            6 => Ok(Self::DevicePaired),
            8 => Ok(Self::MessagePlist),
            _ => Err(ParseError::InvalidData(format!(
                "`{value}` is not a valid usbmux message type"
            ))),
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum UsbMuxResult {
    OK = 0,
    BadCommand = 1,
    BadDeviceOrNoSuchFile = 2,
    ConnectionRefused = 3,
    BadVersion = 6,
    InvalidInput = 22,
}

impl TryFrom<u32> for UsbMuxResult {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::OK),
            1 => Ok(Self::BadCommand),
            2 => Ok(Self::BadDeviceOrNoSuchFile),
            3 => Ok(Self::ConnectionRefused),
            6 => Ok(Self::BadVersion),
            22 => Ok(Self::InvalidInput),
            _ => Err(ParseError::InvalidData(format!(
                "`{value}` is not a valid usbmux result"
            ))),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct UsbMuxCommon {
    #[serde(rename = "BundleID")]
    pub bundle_id: Option<String>,

    pub client_version_string: Option<String>,
    pub conn_type: Option<u8>,

    #[serde(rename = "ProcessID")]
    pub process_id: Option<u32>,

    pub prog_name: Option<String>,

    #[serde(rename = "kLibUSBMuxVersion")]
    pub libusbmux_version: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "MessageType")]
pub enum UsbMuxRequest {
    Listen {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },
    ListDevices {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },
    ListListeners {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },
    ReadBUID {
        #[serde(flatten)]
        common: UsbMuxCommon,
    },
    ReadPairRecord {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "PairRecordID")]
        pair_record_id: String,
    },
    SavePairRecord {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "PairRecordID")]
        pair_record_id: String,

        #[serde(rename = "PairRecordData")]
        pair_record_data: plist::Data,

        #[serde(rename = "DeviceID")]
        device_id: Option<u64>,
    },
    DeletePairRecord {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "PairRecordID")]
        pair_record_id: String,
    },
    Connect {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(rename = "DeviceID")]
        device_id: u64,

        #[serde(rename = "PortNumber", deserialize_with = "deserialize_port_number")]
        port: u16,
    },
    AddDevice {
        #[serde(flatten)]
        common: UsbMuxCommon,

        #[serde(alias = "IP", alias = "ip", alias = "IPAddress")]
        ip: IpAddr,
        #[serde(alias = "UDID", alias = "SerialNumber")]
        udid: String,

        #[serde(alias = "Force")]
        force: bool,
    },
}

fn deserialize_port_number<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let value = plist::Value::deserialize(deserializer)?;

    if let Some(ui) = value.as_unsigned_integer() {
        Ok((ui as u16).to_be())
    } else if let Some(si) = value.as_signed_integer() {
        Ok((si as u16).to_be())
    } else {
        Err(serde::de::Error::custom(
            "PortNumber is neither a signed number nor an unsigned number",
        ))
    }
}

impl UsbMuxRequest {
    fn encode_common(common: UsbMuxCommon) -> plist::Value {
        plist_macro::plist!({
            "BundleID":? common.bundle_id,
            "ClientVersionString":? common.client_version_string,
            "ConnType":? common.conn_type.map(|n| n as u16),
            "ProcessID":? common.process_id,
            "ProgramName":? common.prog_name,
            "kLibUSBMuxVersion":? common.libusbmux_version.map(|n| n as u16)
        })
    }
    fn message_type(&self) -> &'static str {
        match self {
            Self::Listen { .. } => "Listen",
            Self::ListDevices { .. } => "ListDevices",
            Self::ListListeners { .. } => "ListListeners",
            Self::ReadBUID { .. } => "ReadBUID",
            Self::ReadPairRecord { .. } => "ReadPairRecord",
            Self::SavePairRecord { .. } => "SavePairRecord",
            Self::DeletePairRecord { .. } => "DeletePairRecord",
            Self::Connect { .. } => "Connect",
            Self::AddDevice { .. } => "AddDevice",
        }
    }
    pub fn into_plist(self) -> plist::Value {
        let message_type = Self::message_type(&self);

        match self {
            Self::Listen { common }
            | Self::ListListeners { common }
            | Self::ListDevices { common }
            | Self::ReadBUID { common } => {
                plist_macro::plist!({
                    "MessageType": message_type,
                    :<Self::encode_common(common)
                })
            }
            Self::ReadPairRecord {
                common,
                pair_record_id,
            }
            | Self::DeletePairRecord {
                common,
                pair_record_id,
            } => {
                plist_macro::plist!({
                    "MessageType": message_type,
                    "PairRecordID": pair_record_id,
                    :<Self::encode_common(common)
                })
            }
            Self::SavePairRecord {
                common,
                pair_record_id,
                pair_record_data,
                device_id,
            } => {
                plist_macro::plist!({
                    "MessageType": message_type,
                    "PairRecordID": pair_record_id,
                    "PairRecordData": plist::Value::Data(pair_record_data.into()),
                    "DeviceID":? device_id,
                    :<Self::encode_common(common)
                })
            }
            Self::Connect {
                common,
                device_id,
                port,
            } => {
                plist_macro::plist!({
                    "MessageType": message_type,
                    "DeviceID": device_id,
                    "PortNumber": port,
                    :<Self::encode_common(common)
                })
            }
            Self::AddDevice {
                common,
                ip,
                udid,
                force,
            } => {
                plist_macro::plist!({
                    "MessageType": message_type,
                    "IP": ip.to_string(),
                    "UDID": udid,
                    "Force": force,
                    :<Self::encode_common(common)
                })
            }
        }
    }
}

impl UsbMuxCommon {
    pub fn builder() -> Self {
        Self::default()
    }

    pub fn libusbmux_version(mut self, libusbmux_version: u8) -> Self {
        self.libusbmux_version = Some(libusbmux_version);
        self
    }

    pub fn process_id(mut self, process_id: u32) -> Self {
        self.process_id = Some(process_id);
        self
    }

    pub fn connection_type(mut self, connection_type: u8) -> Self {
        self.conn_type = Some(connection_type);
        self
    }

    pub fn client_version_string(mut self, client_version_string: &str) -> Self {
        self.client_version_string = Some(client_version_string.to_string());
        self
    }

    pub fn bundle_id(mut self, bundle_id: &str) -> Self {
        self.bundle_id = Some(bundle_id.to_string());
        self
    }

    pub fn program_name(mut self, program_name: &str) -> Self {
        self.prog_name = Some(program_name.to_string());
        self
    }
}
