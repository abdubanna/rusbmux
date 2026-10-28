use idevice::{
    IdeviceError, IdeviceService,
    pairing_file::PairingFile,
    provider::IdeviceProvider,
    services::{lockdown::LockdownClient, notification_proxy::NotificationProxyClient},
};
use tracing::{debug, warn};

use crate::{device::usb::UsbDeviceShared, error::RusbmuxError, provider::RusbmuxProvider};

const INSECURE_NOTIFICATION_PROXY: &str = "com.apple.mobile.insecure_notification_proxy";
const REQUEST_PAIR: &str = "com.apple.mobile.lockdown.request_pair";
const REQUEST_HOST_BUID: &str = "com.apple.mobile.lockdown.request_host_buid";

pub async fn preflight(
    device: UsbDeviceShared,
    pairing_file: Option<&PairingFile>,
    system_buid: &str,
) -> Result<Option<PairingFile>, RusbmuxError> {
    let provider = RusbmuxProvider::new_from(
        device,
        "rusbmux-preflight".to_string(),
        Some(system_buid.to_string()),
    );

    let mut lockdown = LockdownClient::connect(&provider).await?;

    if lockdown.idevice.get_type().await? != "com.apple.mobile.lockdown" {
        // restore mode
        return Ok(None);
    }

    if let Some(pairing_file) = pairing_file {
        match lockdown.start_session(pairing_file).await {
            Ok(_) => return Ok(None),
            Err(IdeviceError::InvalidHostID) => {}
            // something is wrong with the pairing file
            Err(
                err @ (IdeviceError::Rustls(_)
                | IdeviceError::TlsBuilderFailed(_)
                | IdeviceError::PemParseFailed(_)),
            ) => {
                debug!(
                    %err,
                    "Failed to start a session, the pairing file might be corrupted"
                );
                lockdown = LockdownClient::connect(&provider).await?;
            }
            Err(err) => return Err(err.into()),
        }
    }

    let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();

    lockdown
        .set_value("UntrustedHostBUID", system_buid.into(), None)
        .await?;

    let product_version = lockdown
        .get_value(Some("ProductVersion"), None)
        .await?
        .into_string()
        .ok_or(RusbmuxError::InvalidData(
            "`ProductVersion` is not a string",
        ))?;

    let product_version_major = product_version
        .split('.')
        .next()
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or(RusbmuxError::InvalidData("`ProductVersion` is not valid"))?;

    let device_class = lockdown
        .get_value(Some("DeviceClass"), None)
        .await?
        .into_string()
        .ok_or(RusbmuxError::InvalidData("`DeviceClass` is not a string"))?;

    let support_notifications = (matches!(device_class.as_str(), "iPhone" | "iPad")
        && product_version_major >= 7)
        || (device_class == "Watch" && product_version_major >= 2)
        || (device_class == "AppleTV" && product_version_major >= 9);

    // TODO: test on an old device
    let pairing_file = match lockdown.pair_once(&host_id, system_buid, None).await {
        Ok(pairing_file) => pairing_file,
        Err(
            err @ (IdeviceError::PasswordProtected | IdeviceError::PairingDialogResponsePending),
        ) => {
            if support_notifications {
                let mut notifications = connect_notifications(&provider, &mut lockdown).await?;

                wait_for_pair_request(&mut lockdown, &mut notifications, system_buid, &host_id)
                    .await?;
                lockdown.pair_once(host_id, system_buid, None).await?
            } else {
                warn!(%err, "Failed to pair");
                warn!("Device doesn't support notification proxy, trust the host and retry again");
                return Ok(None);
            }
        }
        Err(err) => return Err(err.into()),
    };

    debug!("Verifying the pairing file");
    lockdown.start_session(&pairing_file).await?;

    // TODO: ValidatePair

    debug!("Preflight succeeded");

    Ok(Some(pairing_file))
}

async fn connect_notifications(
    provider: &RusbmuxProvider,
    lockdown: &mut LockdownClient,
) -> Result<NotificationProxyClient, RusbmuxError> {
    let (port, _) = lockdown.start_service(INSECURE_NOTIFICATION_PROXY).await?;
    let mut notifications = NotificationProxyClient::new(provider.connect(port).await?);

    notifications
        .observe_notifications(&[REQUEST_PAIR, REQUEST_HOST_BUID])
        .await?;

    Ok(notifications)
}

async fn wait_for_pair_request(
    lockdown: &mut LockdownClient,
    notifications: &mut NotificationProxyClient,
    system_buid: &str,
    host_id: &str,
) -> Result<(), RusbmuxError> {
    loop {
        tokio::select! {
            notification = notifications.receive_notification() => {
                match notification?.as_str() {
                    REQUEST_PAIR => return Ok(()),
                    REQUEST_HOST_BUID => {
                        lockdown
                            .set_value("UntrustedHostBUID", system_buid.into(), None)
                            .await?;
                    }
                    _ => {}

                }
            },
            // break the preflight task if the pairing got denied
            //
            // TODO: is this a problem?
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                if let Err(err @ IdeviceError::UserDeniedPairing) = lockdown.pair_once(host_id, system_buid, None).await {
                    debug!("User denied pairing, cancelling preflight");
                    break Err(RusbmuxError::Idevice(err));
                }
            }
        }
    }
}
