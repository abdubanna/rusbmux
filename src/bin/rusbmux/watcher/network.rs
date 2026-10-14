use std::path::Path;

use futures::StreamExt;
use rusbmux::{
    error::RusbmuxError,
    watcher::{NetworkEvent, watch_network},
};
use tracing::{debug, error, warn};

use crate::{handler::LOCKDOWN_PATH, watcher::DeviceEvent};

pub async fn watch_network_daemon() -> Result<(), RusbmuxError> {
    let mut watcher = watch_network(Path::new(LOCKDOWN_PATH));

    while let Some(event) = watcher.next().await {
        match event {
            Ok(NetworkEvent::Connected(dev)) => {
                debug!("Adding a new network device");

                let udid = dev.udid().to_string();

                // if the device broadcasted twice, and the first is still connecting to the heartbeat, this
                // would get the device connect twice
                //
                // TODO: check on the resolved service it self
                if crate::utils::has_network_with_udid(&udid) {
                    debug!(udid = dev.udid().as_ref(), "Device already added, skipping");
                    continue;
                }

                let id = dev.id();
                super::CONNECTED_DEVICES.insert(id, dev);

                // prefer usb devices over network devices for the same udid
                //
                // skip hotplug notifications when the device is already connected via usb
                //
                // it would get notified only if the usb is disconnected
                if !crate::utils::has_usb_with_udid(&udid) {
                    let _ = super::get_hotplug_event_tx()
                        .await
                        .send(super::DeviceEvent::Attached { id });
                }
            }
            Ok(NetworkEvent::Disconnected(id)) => {
                debug!(id, "Removing network device");

                let Some((_, device)) = super::CONNECTED_DEVICES.remove(&id) else {
                    warn!(
                        id,
                        "Received a network device disconnect that never connected"
                    );
                    continue;
                };
                let _ = device.shutdown().await;

                // dedup
                if !crate::utils::has_usb_with_udid(&device.udid()) {
                    let _ = crate::watcher::get_hotplug_event_tx()
                        .await
                        .send(DeviceEvent::Detached { id });
                }
            }
            // NOTE: maybe it should retry
            Err(err) if matches!(err, RusbmuxError::Mdns(_)) => return Err(err),

            Err(err) => {
                error!(%err, "Couldn't create a new network device");
            }
        }
    }

    Ok(())
}
