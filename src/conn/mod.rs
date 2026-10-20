mod network;
mod usb;

use crate::{device::ConnectionType, error::RusbmuxError};
pub use network::NetworkDeviceConn;
pub(crate) use usb::ConnState;
pub use usb::UsbDeviceConn;

pub enum DeviceConn {
    Network(NetworkDeviceConn),
    Usb(UsbDeviceConn),
}

impl DeviceConn {
    pub const fn connection_type(&self) -> ConnectionType {
        match self {
            Self::Usb(_) => ConnectionType::Usb,
            Self::Network(_) => ConnectionType::Network,
        }
    }

    pub const fn as_network(&mut self) -> Option<&mut NetworkDeviceConn> {
        if let Self::Network(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    pub const fn as_usb(&mut self) -> Option<&mut UsbDeviceConn> {
        if let Self::Usb(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    pub fn device_id(&self) -> u64 {
        match self {
            Self::Usb(dev) => dev.device_id(),
            Self::Network(dev) => dev.device_id(),
        }
    }

    pub fn destination_port(&self) -> u16 {
        match self {
            Self::Usb(dev) => dev.destination_port(),
            Self::Network(dev) => dev.destination_port(),
        }
    }

    pub async fn close(&mut self) -> Result<(), RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.close().await,
            Self::Network(_) => Ok(()),
        }
    }

    pub async fn wait_shutdown(&self) {
        match self {
            Self::Usb(dev) => dev.wait_shutdown().await,
            Self::Network(dev) => dev.wait_shutdown().await,
        }
    }
}
