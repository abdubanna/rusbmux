use std::{net::IpAddr, path::Path};

use idevice::pairing_file::PairingFile;
use tracing::{debug, info, warn};

use rusbmux::{
    device::Device, error::RusbmuxError, usb_backend::next_device_id, usbmux::UsbMuxResult,
};

use crate::{
    daemon::ConnectionStream,
    watcher::{CONNECTED_DEVICES, DeviceEvent, get_hotplug_event_tx},
};

use super::{LOCKDOWN_PATH, send_result};

pub async fn handle_add_device(
    client: &mut ConnectionStream,
    ip: IpAddr,
    udid: String,
    force: bool,
    tag: u32,
) -> Result<(), RusbmuxError> {
    match add_device(ip, udid, force).await {
        Ok(()) => send_result(client, UsbMuxResult::OK, tag).await?,
        Err(err) => {
            let code = match &err {
                RusbmuxError::IO(io)
                    if io.kind() == std::io::ErrorKind::NotFound
                        || io.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    UsbMuxResult::BadDeviceOrNoSuchFile
                }
                RusbmuxError::DeviceNotFound(_) => UsbMuxResult::BadDeviceOrNoSuchFile,
                RusbmuxError::Idevice(_) => UsbMuxResult::InvalidInput,
                _ => UsbMuxResult::ConnectionRefused,
            };
            send_result(client, code, tag).await?;
            return Err(err);
        }
    }

    Ok(())
}

async fn add_device(ip: IpAddr, udid: String, force: bool) -> Result<(), RusbmuxError> {
    super::check_pair_record_id(&udid)?;

    let path = Path::new(LOCKDOWN_PATH).join(format!("{udid}.plist"));
    let pairing_file_bytes = tokio::fs::read(path).await?;

    let mut pairing_file = PairingFile::from_bytes(&pairing_file_bytes)?;
    pairing_file.udid = Some(udid.clone());

    let old_ndev = match crate::utils::find_network_id_by_udid(Some(udid.as_str())) {
        Some(ndev_id) => {
            debug!(udid, "Device already exists");
            if !force {
                return Ok(());
            }
            debug!(udid, "Adding device anyway");

            let _ = get_hotplug_event_tx()
                .await
                .send(DeviceEvent::Detached { id: ndev_id });

            CONNECTED_DEVICES.remove(&ndev_id)
        }
        None => None,
    };

    let id = next_device_id();

    let device =
        match Device::new_network(id, (ip, None), None, "manual".to_string(), pairing_file).await {
            Ok(d) => d,
            Err(err) => {
                warn!(%err, udid, "Failed to add the manual network device");

                if let Some((id, ndev)) = old_ndev {
                    debug!(udid, "Restoring the previous device");
                    CONNECTED_DEVICES.insert(id, ndev);
                }

                return Err(err);
            }
        };

    CONNECTED_DEVICES.insert(id, device);

    // dedup
    if !crate::utils::has_usb_with_udid(Some(udid.as_str())) {
        let _ = get_hotplug_event_tx()
            .await
            .send(DeviceEvent::Attached { id });
        info!(id, %udid, %ip, "Manually added network device");
    } else {
        info!(id, %udid, "Network device tracked but not broadcast (USB preferred)");
    }

    Ok(())
}
