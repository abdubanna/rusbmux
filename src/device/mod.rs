pub(crate) mod core;
pub mod network;
pub(crate) mod packet_router;
mod power_assertion;
pub mod usb;
use std::net::IpAddr;

use crossfire::{MAsyncTx, mpsc};
use idevice::pairing_file::PairingFile;
use network::NetworkDevice;
use usb::UsbDevice;

use crate::{
    conn::DeviceConn, device::usb::IODisconnectedDevice, error::RusbmuxError,
    usb_backend::AnyDeviceInfo,
};

#[derive(Debug)]
pub enum Device {
    Network(NetworkDevice),
    Usb(UsbDevice),
}

#[derive(Debug)]
pub enum ConnectionType {
    Usb,
    Network,
}

impl Device {
    #[must_use]
    pub const fn connection_type(&self) -> ConnectionType {
        match self {
            Self::Usb(_) => ConnectionType::Usb,
            Self::Network(_) => ConnectionType::Network,
        }
    }

    #[inline]
    pub const fn is_usb(&self) -> bool {
        matches!(self.connection_type(), ConnectionType::Usb)
    }

    #[inline]
    pub const fn is_network(&self) -> bool {
        matches!(self.connection_type(), ConnectionType::Network)
    }

    pub fn id(&self) -> u64 {
        match self {
            Self::Network(dev) => dev.id(),
            Self::Usb(dev) => dev.id(),
        }
    }

    pub fn udid(&self) -> Option<&str> {
        match self {
            Self::Network(dev) => dev.udid.as_deref(),
            Self::Usb(dev) => dev.udid(),
        }
    }

    pub async fn new_usb(info: AnyDeviceInfo, id: u64) -> Result<Self, RusbmuxError> {
        Ok(Self::Usb(UsbDevice::new(info, id).await?))
    }

    pub async fn new_usb_with_disconnect_tx(
        info: AnyDeviceInfo,
        id: u64,
        disconnected_tx: Option<MAsyncTx<mpsc::Array<IODisconnectedDevice>>>,
    ) -> Result<Self, RusbmuxError> {
        Ok(Self::Usb(
            UsbDevice::new_with_disconnect_tx(info, id, disconnected_tx).await?,
        ))
    }

    pub async fn new_network(
        id: u64,
        addr: (IpAddr, Option<IpAddr>),
        scope_id: Option<u32>,
        service_name: String,
        pairing_file: PairingFile,
    ) -> Result<Self, RusbmuxError> {
        Ok(Self::Network(
            NetworkDevice::new(id, addr, scope_id, service_name, pairing_file).await?,
        ))
    }

    pub async fn shutdown(&self) -> Result<(), RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.shutdown().await?,
            Self::Network(dev) => dev.shutdown(),
        }

        Ok(())
    }

    pub async fn connect(&self, destination_port: u16) -> Result<DeviceConn, RusbmuxError> {
        match self {
            Self::Usb(dev) => dev.connect(destination_port).await.map(DeviceConn::Usb),
            Self::Network(dev) => dev.connect(destination_port).await.map(DeviceConn::Network),
        }
    }

    #[must_use]
    pub const fn as_network(&self) -> Option<&NetworkDevice> {
        if let Self::Network(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_usb(&self) -> Option<&UsbDevice> {
        if let Self::Usb(dev) = self {
            Some(dev)
        } else {
            None
        }
    }

    pub fn create_device_attached(&self) -> plist::Value {
        match self {
            Self::Usb(dev) => dev.create_device_attached(),
            Self::Network(dev) => dev.create_device_attached(),
        }
    }
}
