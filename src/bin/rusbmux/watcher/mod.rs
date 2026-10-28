mod network;
mod preflight;
mod usb;
pub use network::watch_network_daemon;
pub use usb::watch_usb_daemon;

use dashmap::DashMap;
use rusbmux::{
    device::{ConnectionType, Device},
    error::RusbmuxError,
};
use std::sync::LazyLock;
use tokio::sync::{OnceCell, broadcast};

/// has the currently connected devices with it's corresponding idevice id
///
/// devices are pushed to it whenever a device is connected, and removed once the device is removed
pub static CONNECTED_DEVICES: LazyLock<DashMap<u64, Device>> = LazyLock::new(DashMap::new);

/// a channel used for hotplug events, once a device is connected it gets broadcasted to all it's
/// subscribers
///
/// it only sends the device id, the device it self is stored in
/// `CONNECTED_DEVICES`
pub static HOTPLUG_EVENT_TX: OnceCell<broadcast::Sender<DeviceEvent>> = OnceCell::const_new();

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Attached { id: u64 },
    Detached { id: u64 },
}

impl std::fmt::Display for DeviceEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Attached { id } => write!(f, "Attached({id})"),
            Self::Detached { id } => write!(f, "Detached({id})"),
        }
    }
}

#[inline]
pub async fn get_hotplug_event_tx() -> &'static broadcast::Sender<DeviceEvent> {
    HOTPLUG_EVENT_TX
        .get_or_init(|| async { broadcast::channel::<DeviceEvent>(32).0 })
        .await
}

/// Removes the device from the connected devices and shut it down
pub async fn remove_device(id: u64) -> Result<Device, RusbmuxError> {
    let (_, mut device) = crate::watcher::CONNECTED_DEVICES
        .remove(&id)
        .ok_or(RusbmuxError::DeviceNotFound(id))?;
    // shutting down the device would also send an RST to the connections opened by the device
    //
    // this is suppose to be used while the device is connected, but the `remove_device`
    // function mostly gets called after the device have been physically/io disconnected
    //
    // so we don't want to error out in here
    let _ = device.shutdown().await;

    // if the removed device is a usb, and there's a network device connected with the same
    // udid, it would notify the apps (whoever doing a `Listen`)
    // that the network device is now connected
    //
    // or if the removed device is a network, and there's NO usb device connected with the same
    // udid, it would notify the apps with a detached event
    // if otherwise there's a usb device connected, it would skip, because it's already detached
    //
    // this is to dedup and expose only one device (either usb or network, not both, while also
    // prefering usb over network)
    match device.connection_type() {
        // the removed device is a usb, and there's a network device with the same udid
        ConnectionType::Usb => {
            if let Some(ndev_id) = crate::utils::find_network_id_by_udid(device.udid()) {
                let _ = get_hotplug_event_tx()
                    .await
                    .send(DeviceEvent::Attached { id: ndev_id });
            }

            let _ = get_hotplug_event_tx()
                .await
                .send(DeviceEvent::Detached { id });
        }

        // the network device is also connected as usb, so skip sending the detached event
        //
        // because the network device is already detached from the listener (dedup purposes)
        ConnectionType::Network if crate::utils::has_usb_with_udid(device.udid()) => {}

        // the network device is not also connected as usb
        //
        // this is a duplication from the network watcher, but sometimes mdns doesn't broadcast a
        // removal, so this will be fired if we don't get a heartbeat response from the device
        // if it did broadcast a removal, then function will return with device not found, because
        // the network watcher already removed it
        ConnectionType::Network => {
            let _ = get_hotplug_event_tx()
                .await
                .send(DeviceEvent::Detached { id: device.id() });
        }
    }

    Ok(device)
}
