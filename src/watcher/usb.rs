use std::time::Duration;

use crossfire::mpsc;
use futures::{Stream, StreamExt};
use tokio::time::Instant;

use crate::{
    device::{Device, usb::IODisconnectedDevice},
    error::RusbmuxError,
    usb_backend::{self, UsbBackend},
};

use tracing::{debug, error};

#[derive(Debug)]
pub enum UsbEvent {
    Connected {
        device: Device,
        id: u64,
    },
    Disconnected {
        id: u64,

        /// the hash of the device info if it's an io disconnect
        opaque_id: Option<u64>,

        /// devices sometimes disconnect from the USB endpoints (not from hotplug)
        io_disconnect: bool,
    },
}

pub fn watch_usb(
    backend: &impl UsbBackend,
) -> std::pin::Pin<Box<impl Stream<Item = Result<UsbEvent, RusbmuxError>>>> {
    let (disconnected_tx, disconnected_rx) = mpsc::bounded_async(32);

    Box::pin(async_stream::stream! {
        let mut devices_hotplug = backend
            .watch_devices()
            .await?;

        loop {
            tokio::select! {
                Some(event) = devices_hotplug.next() =>  {
                    debug!("{event:#?}");

                    match event {
                        Ok(usb_backend::Event::Connected(device_info, id)) => {
                            let opaque_id = device_info.opaque_id();
                            let device = match Device::new_usb_with_disconnect_tx(device_info, id, Some(disconnected_tx.clone())).await {
                                Ok(device) => Ok(device),
                                Err(first_error) => {
                                    let deadline = Instant::now() + Duration::from_secs(3);

                                    loop {
                                        if Instant::now() >= deadline {
                                            break Err(first_error);
                                        }

                                        let device_info = backend
                                            .list_devices()
                                            .await
                                            .into_iter()
                                            .find(|device| device.opaque_id() == opaque_id);

                                        if let Some(device_info) = device_info
                                            && let Ok(device) = Device::new_usb_with_disconnect_tx(device_info, id, Some(disconnected_tx.clone())).await
                                        {
                                            break Ok(device);
                                        }

                                        tokio::time::sleep(Duration::from_millis(100)).await;
                                    }
                                }
                            };

                            match device {
                                Ok(device) => yield Ok(UsbEvent::Connected { device, id }),
                                Err(err) => {
                                    yield Err(err)
                                }
                            }
                        }
                        Ok(usb_backend::Event::Disconnected(id)) => {
                            yield Ok(
                                UsbEvent::Disconnected { id, opaque_id: None, io_disconnect: false }
                            );
                        }
                        Err(err) => {
                            error!(%err, "Hotplug error");
                            yield Err(err)
                        },

                    }
                }
                Ok(IODisconnectedDevice { id, opaque_id }) = disconnected_rx.recv() => {
                    yield Ok(UsbEvent::Disconnected {
                        id,
                        opaque_id: Some(opaque_id),
                        io_disconnect: true
                    });

                    debug!(opaque_id, "Trying to reopen closed device");

                    // try to bring it back
                    let device_info = backend
                        .list_devices()
                        .await
                        .into_iter()
                        .find(|device| device.opaque_id() == opaque_id);

                    if let Some(device_info) = device_info {
                        match Device::new_usb_with_disconnect_tx(
                            device_info,
                            id,
                            Some(disconnected_tx.clone())
                        ).await {
                            Ok(device) => {
                                yield Ok(UsbEvent::Connected { device, id });
                            }
                            Err(err) => {
                                error!(%err, "Failed to reopen IO-disconnected device");
                                yield Err(err);
                            }
                        }
                    }
                }
            }
        }
    })
}
