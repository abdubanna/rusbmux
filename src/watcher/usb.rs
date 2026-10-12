use std::time::Duration;

use crossfire::mpsc;
use futures_lite::{Stream, StreamExt};
use tokio::time::Instant;

use crate::{
    device::{Device, usb::IODisconnectedDevice},
    error::RusbmuxError,
    usb_backend::{self, UsbBackend},
};

use super::{CONNECTED_DEVICES, DeviceEvent};
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

pub async fn watch_usb_daemon(backend: impl UsbBackend) {
    let hotplug_event_tx = super::get_hotplug_event_tx().await;

    let mut usb_hotplug = watch_usb(&backend);

    while let Some(event) = usb_hotplug.next().await {
        match event {
            Ok(UsbEvent::Connected { device, id }) => {
                if let Some(ndev_id) = crate::utils::find_network_id_by_udid(&device.udid()) {
                    let _ = hotplug_event_tx.send(DeviceEvent::Detached { id });
                }

                // A device may emit multiple connect events (especially during boot), and the
                // initial connection may not receive a matching disconnect event. Remove any
                // stale entry before registering the new connection.
                CONNECTED_DEVICES.retain(|_, dev| dev.is_network() || dev.udid() != device.udid());

                super::preflight::spawn(device.as_usb().unwrap().shared());
                CONNECTED_DEVICES.insert(id, device);

                let _ = hotplug_event_tx.send(DeviceEvent::Attached { id });
            }

            Ok(UsbEvent::Disconnected { id, .. }) => match super::remove_device(id).await {
                Ok(_) => {}
                Err(RusbmuxError::DeviceNotFound(_)) => {}
                Err(err) => error!(%err, "Failed to remove disconnected device"),
            },
            Err(err) => {
                error!(%err, "Failed to create a new device");
            }
        }
    }
}
