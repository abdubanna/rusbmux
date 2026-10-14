//! **rusbmux** is a modern, drop-in replacement for `usbmuxd`, written in pure Rust.
//!
//! It ships both a daemon binary (`bin`) and a library. The library exposes the
//! same core: open an Apple device over USB, multiplex it, and let applications
//! talk to it — no socket, no daemon process.
//!
//! # Using rusbmux with idevice (library mode)
//!
//! rusbmux's USB connection is exposed through [`provider::RusbmuxProvider`],
//! which implements `idevice::provider::IdeviceProvider`. Because every service
//! client in the **idevice** crate connects through that single
//! trait, an `RusbmuxProvider` can be handed straight to any of them — over USB,
//! without a `usbmuxd` daemon.
//!
//! ```no_run
//! # use idevice::{IdeviceService, services::lockdown::LockdownClient};
//! # use rusbmux::{
//! #    watcher::{UsbEvent, watch_usb},
//! #    provider::RusbmuxProvider,
//! #    device::Device
//! # };
//! # use futures::stream::StreamExt;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut usb_hotplug = watch_usb(&rusbmux::usb_backend::DEFAULT_BACKEND);
//!
//! let UsbEvent::Connected {
//!     device: Device::Usb(device),
//!     id,
//! } = usb_hotplug.next().await.unwrap().unwrap()
//! else {
//!     return Err("no USB device connected".into());
//! };
//!
//! // rusbmux as an idevice provider — direct USB, no daemon
//! let provider = RusbmuxProvider::new(&device, "my-app".to_string(), None);
//!
//! // any idevice service client can now connect over USB
//! let mut lockdown = LockdownClient::connect(&provider).await?;
//! let name = lockdown.get_value(Some("DeviceName"), None).await?;
//!
//! println!("Device name: {name:?}");
//! #    Ok(())
//! # }
//! ```
//!
//! # Notes
//!
//! ### Exclusive USB ownership
//!
//! Library mode talks to the device over its raw
//! bulk endpoints, which can only be opened by one process. Stop the `rusbmux`
//! daemon (or `usbmuxd`) before using the library.
//!
//! A more flexable modes are planned, including coexisting
//!
//! ### Pairing
//!
//! Services that start a TLS session need a pairing file.
//!
//! [`provider::RusbmuxProvider::preflight`]
//! will check and pair and wait for the trust dialog if needed, and
//! and sets the new pairing file.
//!
//! Prefer [`provider::RusbmuxProvider::set_pairing_file`] when you
//! already have a pairing file.
//!
//! A better choice is to set the pairing file AND do preflight, if the pairing file
//! is valid, preflight is skipped, if not, it would generate a new one for you, you can
//! then check if preflight did generate a new pairing file (by the returned boolean),
//! and get it and save it somewhere for later.

#[cfg(all(not(feature = "rusb"), not(feature = "nusb")))]
compile_error!("Need either the `rusb` feature or `nusb`");

pub mod conn;

pub mod device;
pub mod error;

mod parser;
pub use parser::usbmux;

pub mod provider;

#[cfg(any(feature = "rusb", feature = "nusb"))]
pub mod usb_backend;

pub(crate) mod utils;
pub mod watcher;
