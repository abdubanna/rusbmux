use std::path::Path;

use futures::StreamExt;
use idevice::pairing_file::PairingFile;
use rusbmux::{
    error::RusbmuxError,
    watcher::{NetworkEvent, PairingFiles, watch_network},
};
use tracing::{debug, error, warn};

use crate::{handler::LOCKDOWN_PATH, watcher::DeviceEvent};

pub async fn watch_network_daemon() -> Result<(), RusbmuxError> {
    let mut watcher = watch_network(PairingFiles::dynamic(get_saved_pairing_files));

    while let Some(event) = watcher.next().await {
        match event {
            Ok(NetworkEvent::Connected(dev)) => {
                debug!("Adding a new network device");

                let udid = dev.udid();

                // FIXME: if the device broadcasted twice, and the first is still connecting to the heartbeat, this
                // would get the device to connect twice
                if crate::utils::has_network_with_udid(udid) {
                    debug!(udid = dev.udid().as_ref(), "Device already added, skipping");
                    continue;
                }

                let id = dev.id();

                let has_usb = crate::utils::has_usb_with_udid(udid);

                super::CONNECTED_DEVICES.insert(id, dev);

                // prefer usb devices over network devices for the same udid
                //
                // skip hotplug notifications when the device is already connected via usb
                //
                // it would get notified only if the usb is disconnected
                if !has_usb {
                    let _ = super::get_hotplug_event_tx()
                        .await
                        .send(super::DeviceEvent::Attached { id });
                }
            }
            Ok(NetworkEvent::Disconnected(id)) => {
                debug!(id, "Removing network device");

                let Some((_, mut device)) = super::CONNECTED_DEVICES.remove(&id) else {
                    warn!(
                        id,
                        "Received a network device disconnect that never connected"
                    );
                    continue;
                };
                let _ = device.shutdown().await;

                // dedup
                if !crate::utils::has_usb_with_udid(device.udid()) {
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

fn get_saved_pairing_files() -> Vec<PairingFile> {
    let entries = match Path::new(LOCKDOWN_PATH).read_dir() {
        Ok(dirs) => dirs,
        Err(err) => {
            debug!(%err, "Failed to read the directories of `{LOCKDOWN_PATH:?}`");
            return Vec::new();
        }
    };

    entries
        .flatten()
        .map(|di| di.path())
        .map(|path| {
            (
                path.file_stem()
                    .and_then(|fstem| fstem.to_str())
                    .map(|s| s.to_string()),
                path,
            )
        })
        .flat_map(|(fstem, path)| {
            let udid = fstem?;

            if udid == "SystemConfiguration" {
                return None;
            }

            let Ok(mut pf) = PairingFile::read_from_file(path) else {
                return None;
            };

            if pf.udid.is_none() {
                pf.udid = Some(udid);
            }

            Some(pf)
        })
        .collect()
}
