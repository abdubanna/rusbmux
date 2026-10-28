use thiserror::Error;

use crate::{device::WriterMsg, parser::usbmux::UsbMuxResult};

#[derive(Debug, Error)]
pub enum RusbmuxError {
    #[cfg(feature = "nusb")]
    #[error("USB error: {0}")]
    USB(#[from] nusb::Error),

    #[error("IO Error: {0}")]
    IO(#[from] std::io::Error),

    #[error("There is no usbmux interface found in the usb")]
    UsbmuxInterfaceNotFound,

    #[error("There is no bulk (in) endpoint found in the interface")]
    BulkInEndpointNotFound,

    #[error("There is no bulk (out) endpoint found in the interface")]
    BulkOutEndpointNotFound,

    #[error("Parsing error: {0}")]
    Parse(#[from] ParseError),

    #[error("Channel error: {0}")]
    Channel(#[from] ChannelError),

    #[error("Received an unexpected packet: {0}")]
    UnexpectedPacket(String),

    #[error("Invalid data: {0}")]
    InvalidData(&'static str),

    #[error("Value not found: {0:?}")]
    ValueNotFound(MissingFields),

    #[error("A device with the id `{0}` was not found")]
    DeviceNotFound(u64),

    #[error("The system probably doesn't support usb hotplug")]
    HotPlugNotSupported,

    #[error("Plist parse error: {0}")]
    Plist(#[from] plist::Error),

    #[error("Ran out of source port for connections")]
    RanOutOfSourcePort,

    #[error("The device rejected the power assertion: {0}")]
    PowerAssertion(String),

    #[error("Idevice error: {0}")]
    Idevice(#[from] idevice::IdeviceError),

    #[cfg(feature = "rusb")]
    #[error("Rusb error: {0}")]
    RusbError(#[from] rusb::Error),

    #[error("mdns error: {0}")]
    Mdns(#[from] mdns_sd::Error),
}

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("Couldn't receive on a channel, error: {0}")]
    UsbRecv(#[source] crossfire::RecvError),

    #[error("Couldn't send a usb packet on the channel")]
    UsbSend(Box<WriterMsg>),
}

#[derive(Debug, Clone, Copy)]
pub enum MissingFields {
    PairRecordID,
    PairRecordData,
    DeviceID,
    PortNumber,
    SystemBUID,
}

impl MissingFields {
    pub fn result_code(&self) -> UsbMuxResult {
        match self {
            Self::PairRecordID | Self::PairRecordData | Self::SystemBUID => {
                UsbMuxResult::InvalidInput
            }
            Self::DeviceID => UsbMuxResult::BadDeviceOrNoSuchFile,
            Self::PortNumber => UsbMuxResult::BadCommand,
        }
    }
}

impl From<crossfire::SendError<WriterMsg>> for RusbmuxError {
    fn from(err: crossfire::SendError<WriterMsg>) -> Self {
        Self::Channel(ChannelError::UsbSend(Box::new(err.0)))
    }
}

impl From<crossfire::TrySendError<WriterMsg>> for RusbmuxError {
    fn from(err: crossfire::TrySendError<WriterMsg>) -> Self {
        Self::Channel(ChannelError::UsbSend(Box::new(err.into_inner())))
    }
}

impl From<crossfire::RecvError> for RusbmuxError {
    fn from(err: crossfire::RecvError) -> Self {
        Self::Channel(ChannelError::UsbRecv(err))
    }
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("IO Error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Plist parse error: {0}")]
    Plist(#[from] plist::Error),

    #[error("Unable to parse tcp header: {0}")]
    TcpHeaderSlice(#[from] etherparse::err::tcp::HeaderSliceError),

    #[error("Invalid data: {0}")]
    InvalidData(String),
}
