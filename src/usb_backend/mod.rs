use std::{
    borrow::Cow,
    fmt::{self, Debug},
    pin::Pin,
    sync::atomic::AtomicU64,
};

#[cfg(feature = "rusb")]
use std::sync::Arc;

use futures::Stream;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{error::RusbmuxError, parser::device_mux::UsbDevicePacket};

#[cfg(feature = "nusb")]
mod nusb;
#[cfg(feature = "nusb")]
pub use nusb::NusbBackend;
#[cfg(feature = "rusb")]
mod rusb;
#[cfg(feature = "rusb")]
pub use rusb::RusbBackend;

// use nusb as the default target if:
//  both nusb and rusb are enabled on a Unix target or
//  only the nusb feature is enabled
#[cfg(any(
    all(feature = "nusb", feature = "rusb", unix),
    all(feature = "nusb", not(feature = "rusb"))
))]
pub const DEFAULT_BACKEND: nusb::NusbBackend = nusb::NusbBackend;

// use rusb as the default target if:
//  both nusb and rusb are enabled on a Windows target or
//  only the rusb feature is enabled
#[cfg(any(
    all(feature = "nusb", feature = "rusb", windows),
    all(feature = "rusb", not(feature = "nusb"))
))]
pub const DEFAULT_BACKEND: rusb::RusbBackend = rusb::RusbBackend;

pub(crate) type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

pub(crate) const APPLE_VID: u16 = 0x5ac;

pub(crate) const APPLE_USBMUX_CLASS: u8 = 255;
pub(crate) const APPLE_USBMUX_SUBCLASS: u8 = 254;
pub(crate) const APPLE_USBMUX_PROTOCOL: u8 = 2;

pub(crate) const MAX_PACKET_SIZE: usize = 48 * 1024;
pub(crate) const MAX_PACKET_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - UsbDevicePacket::HEADERS_LEN_V2;

#[derive(Debug)]
#[non_exhaustive]
pub enum AnyDeviceInfo {
    #[cfg(feature = "nusb")]
    Nusb(::nusb::DeviceInfo),
    #[cfg(feature = "rusb")]
    Rusb(::rusb::Device<::rusb::GlobalContext>),
}

#[derive(Debug)]
#[non_exhaustive]
pub enum AnyDeviceHandle {
    #[cfg(feature = "nusb")]
    Nusb(::nusb::Device),
    #[cfg(feature = "rusb")]
    Rusb {
        handle: Arc<::rusb::DeviceHandle<::rusb::GlobalContext>>,
        end_in: u8,
        end_out: u8,
        max_packet_size: u16,
    },
}

#[non_exhaustive]
pub enum AnyEndpointReader {
    #[cfg(feature = "nusb")]
    Nusb(::nusb::io::EndpointRead<::nusb::transfer::Bulk>),
    #[cfg(feature = "rusb")]
    Rusb(rusb::RusbAsyncReader),
}

#[non_exhaustive]
pub enum AnyEndpointWriter {
    #[cfg(feature = "nusb")]
    Nusb(::nusb::io::EndpointWrite<::nusb::transfer::Bulk>),
    #[cfg(feature = "rusb")]
    Rusb(rusb::RusbAsyncWriter),
}

impl fmt::Debug for AnyEndpointReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(_) => f.debug_tuple("NusbReader").field(&"...").finish(),
            #[cfg(feature = "rusb")]
            Self::Rusb(_) => f.debug_tuple("RusbReader").field(&"...").finish(),
        }
    }
}

impl fmt::Debug for AnyEndpointWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(_) => f.debug_tuple("NusbWriter").field(&"...").finish(),
            #[cfg(feature = "rusb")]
            Self::Rusb(_) => f.debug_tuple("RusbWriter").field(&"...").finish(),
        }
    }
}

impl AsyncRead for AnyEndpointReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            #[cfg(feature = "nusb")]
            Self::Nusb(r) => Pin::new(r).poll_read(cx, buf),
            #[cfg(feature = "rusb")]
            Self::Rusb(r) => Pin::new(r).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for AnyEndpointWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            #[cfg(feature = "nusb")]
            Self::Nusb(w) => Pin::new(w).poll_write(cx, buf),
            #[cfg(feature = "rusb")]
            Self::Rusb(w) => Pin::new(w).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            #[cfg(feature = "nusb")]
            Self::Nusb(w) => Pin::new(w).poll_flush(cx),
            #[cfg(feature = "rusb")]
            Self::Rusb(w) => Pin::new(w).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            #[cfg(feature = "nusb")]
            Self::Nusb(w) => Pin::new(w).poll_shutdown(cx),
            #[cfg(feature = "rusb")]
            Self::Rusb(w) => Pin::new(w).poll_shutdown(cx),
        }
    }
}

impl AnyEndpointWriter {
    pub fn submit_end(&mut self) {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(w) => w.submit_end(),
            #[cfg(feature = "rusb")]
            Self::Rusb(w) => {
                let _ = w.submit_end();
            }
        }
    }
}

impl AnyDeviceInfo {
    pub fn vendor_id(&self) -> u16 {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(info) => info.vendor_id(),
            // since libusb 1.0.16, this function always succeeds
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => dev.device_descriptor().expect("shouldn't fail").vendor_id(),
        }
    }

    pub fn product_id(&self) -> u16 {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(info) => info.product_id(),
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => dev
                .device_descriptor()
                .expect("shouldn't fail")
                .product_id(),
        }
    }

    pub fn udid(&self) -> Option<Cow<'_, str>> {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(info) => info.serial_number().map(crate::utils::to_udid),
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => {
                let desc = dev.device_descriptor().expect("shouldn't fail");
                let serial_number = rusb::get_serial_number(dev, &desc);

                serial_number
                    .map(crate::utils::to_udid_owned)
                    .map(Cow::Owned)
            }
        }
    }

    pub fn speed(&self) -> Option<u64> {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(info) => info.speed().map(crate::utils::nusb_speed_to_number),
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => {
                let speed = dev.speed();

                if matches!(speed, ::rusb::Speed::Unknown) {
                    return None;
                }

                Some(crate::utils::rusb_speed_to_number(speed))
            }
        }
    }

    pub fn location_id(&self) -> u32 {
        match self {
            #[cfg(feature = "nusb")]
            #[allow(unused_variables)]
            Self::Nusb(info) => {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                {
                    (info.busnum() as u32) << 16 | info.device_address() as u32
                }

                #[cfg(target_os = "macos")]
                {
                    info.location_id()
                }

                #[cfg(windows)]
                {
                    0
                }
            }

            #[cfg(feature = "rusb")]
            Self::Rusb(_dev) => {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                {
                    (_dev.bus_number() as u32) << 16 | _dev.address() as u32
                }

                #[cfg(any(target_os = "macos", windows))]
                {
                    0
                }
            }
        }
    }

    pub fn bus_number(&self) -> u8 {
        match self {
            #[cfg(feature = "nusb")]
            #[allow(unused_variables)]
            Self::Nusb(info) => {
                #[cfg(target_os = "linux")]
                {
                    info.busnum()
                }

                #[cfg(any(windows, target_os = "macos"))]
                {
                    0
                }
            }
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => dev.bus_number(),
        }
    }

    pub fn device_address(&self) -> u8 {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(info) => info.device_address(),
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => dev.address(),
        }
    }

    pub fn opaque_id(&self) -> u64 {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(info) => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::hash::DefaultHasher::new();
                info.id().hash(&mut hasher);
                hasher.finish()
            }
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => rusb::opaque_id(dev),
        }
    }

    pub async fn open(&self) -> Result<AnyDeviceHandle, RusbmuxError> {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(info) => Ok(AnyDeviceHandle::Nusb(::nusb::DeviceInfo::open(info).await?)),
            #[cfg(feature = "rusb")]
            Self::Rusb(dev) => {
                let dev_handle = Arc::new(dev.open()?);

                let (end_in, end_out, max_packet_size) =
                    rusb::device_endpoints(dev, Arc::clone(&dev_handle))?;

                Ok(AnyDeviceHandle::Rusb {
                    handle: dev_handle,
                    end_in,
                    end_out,
                    max_packet_size,
                })
            }
        }
    }
}

impl AnyDeviceHandle {
    pub async fn endpoint(&self) -> Result<(AnyEndpointReader, AnyEndpointWriter), RusbmuxError> {
        match self {
            #[cfg(feature = "nusb")]
            Self::Nusb(dev) => {
                let (reader, writer) = nusb::device_endpoints(dev).await?;
                Ok((
                    AnyEndpointReader::Nusb(reader),
                    AnyEndpointWriter::Nusb(writer),
                ))
            }
            #[cfg(feature = "rusb")]
            Self::Rusb {
                handle,
                end_in,
                end_out,
                max_packet_size,
            } => {
                let reader = rusb::RusbAsyncReader::new(Arc::clone(handle), *end_in);
                let writer =
                    rusb::RusbAsyncWriter::new(Arc::clone(handle), *end_out, *max_packet_size);

                Ok((
                    AnyEndpointReader::Rusb(reader),
                    AnyEndpointWriter::Rusb(writer),
                ))
            }
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Connected(AnyDeviceInfo, u64),
    Disconnected(u64),
}

pub trait UsbBackend: Send + Sync + 'static {
    fn list_devices(&self) -> impl std::future::Future<Output = Vec<AnyDeviceInfo>> + Send;
    fn watch_devices(
        &self,
    ) -> impl std::future::Future<
        Output = Result<BoxStream<Result<Event, RusbmuxError>>, RusbmuxError>,
    > + Send;
}

pub(crate) static DEVICE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[inline]
pub fn next_device_id() -> u64 {
    DEVICE_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
