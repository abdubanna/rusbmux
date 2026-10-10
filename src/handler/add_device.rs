use std::{net::IpAddr, path::Path};

use idevice::pairing_file::PairingFile;
use tracing::{debug, info, warn};

use crate::{
    AsyncWriting,
    device::Device,
    error::RusbmuxError,
    handler::{LOCKDOWN_PATH, send_result},
    parser::usbmux::UsbMuxResult,
    usb_backend::next_device_id,
    watcher::{CONNECTED_DEVICES, DeviceEvent, get_hotplug_event_tx},
};

pub async fn handle_add_device(
    client: &mut impl AsyncWriting,
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

    let pairing_file = PairingFile::from_bytes(&pairing_file_bytes)?;

    let old_ndev_id = CONNECTED_DEVICES
        .iter()
        .find(|dev| dev.as_network().is_some_and(|ndev| ndev.udid == udid))
        .map(|d| d.id());

    let old_ndev = match old_ndev_id {
        Some(id) => {
            debug!(udid, "Device already exists");
            if !force {
                return Ok(());
            }
            debug!(udid, "Adding device anyway");

            let _ = get_hotplug_event_tx()
                .await
                .send(DeviceEvent::Detached { id });

            CONNECTED_DEVICES.remove(&id)
        }
        None => None,
    };

    let id = next_device_id();
    let udid_clone = udid.clone();

    let device = match Device::new_network(
        id,
        (ip, None),
        None,
        pairing_file.wifi_mac_address,
        "manual".to_string(),
        udid_clone,
    )
    .await
    {
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
    let has_usb = CONNECTED_DEVICES
        .iter()
        .any(|dev| dev.as_usb().is_some_and(|dev| dev.udid() == udid));

    if !has_usb {
        let _ = get_hotplug_event_tx()
            .await
            .send(DeviceEvent::Attached { id });
        info!(id, %udid, %ip, "Manually added network device");
    } else {
        debug!(id, %udid, "Network device tracked but not broadcast (USB preferred)");
    }

    Ok(())
}
