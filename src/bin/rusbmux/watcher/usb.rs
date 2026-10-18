use futures::StreamExt;
use rusbmux::{
    error::RusbmuxError,
    watcher::{UsbEvent, watch_usb},
};
use tracing::error;

use crate::watcher::DeviceEvent;

pub async fn watch_usb_daemon() {
    let hotplug_event_tx = super::get_hotplug_event_tx().await;

    let mut usb_hotplug = watch_usb(&rusbmux::usb_backend::DEFAULT_BACKEND);

    while let Some(event) = usb_hotplug.next().await {
        match event {
            Ok(UsbEvent::Connected { device, id }) => {
                if let Some(ndev_id) = crate::utils::find_network_id_by_udid(device.udid()) {
                    let _ = hotplug_event_tx.send(DeviceEvent::Detached { id: ndev_id });
                }

                // A device may emit multiple connect events (especially during boot), and the
                // initial connection may not receive a matching disconnect event. Remove any
                // stale entry before registering the new connection.
                super::CONNECTED_DEVICES
                    .retain(|_, dev| dev.is_network() || dev.udid() != device.udid());

                crate::watcher::preflight::spawn(device.as_usb().unwrap().shared());
                super::CONNECTED_DEVICES.insert(id, device);

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
