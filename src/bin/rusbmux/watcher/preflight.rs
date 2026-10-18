use std::{io::ErrorKind, path::Path};

use idevice::pairing_file::PairingFile;
use rusbmux::{device::usb::UsbDeviceShared, error::RusbmuxError};
use tracing::{debug, error};

use crate::handler::LOCKDOWN_PATH;

pub(super) fn spawn(device: UsbDeviceShared) {
    tokio::spawn(async move {
        let canceler = device.canceler();
        tokio::select! {
            Err(err) = run(device) => {
                error!(%err, "Device preflight failed");
            },
            _ = canceler.cancelled() => {}
        }
    });
}

async fn run(device: UsbDeviceShared) -> Result<(), RusbmuxError> {
    let Some(udid) = device.udid() else {
        return Err(RusbmuxError::InvalidData("UDID not found for device"));
    };

    let path = Path::new(LOCKDOWN_PATH).join(format!("{}.plist", udid));

    let pairing_file = match tokio::fs::read(&path).await {
        Ok(data) => {
            let pairing_file = PairingFile::from_bytes(&data);

            match pairing_file {
                Ok(p) => Some(p),
                Err(err) => {
                    debug!(%err, "Failed to parse pairing file");
                    tokio::fs::remove_file(&path).await?;
                    None
                }
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };

    let system_buid = crate::utils::read_system_buid().await?;

    let pairing_file = rusbmux::watcher::preflight(device, pairing_file, &system_buid).await?;

    if let Some(pf) = pairing_file {
        tokio::fs::write(path, pf.serialize()?).await?;
    }

    Ok(())
}
