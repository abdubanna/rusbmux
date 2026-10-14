use std::path::Path;

use rusbmux::{
    device::Device,
    error::{MissingFields, RusbmuxError},
};

use crate::handler::LOCKDOWN_PATH;

pub fn is_disconnect_io(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

pub fn is_disconnect(err: &RusbmuxError) -> bool {
    if let RusbmuxError::IO(io_err) = err {
        return is_disconnect_io(io_err);
    };

    false
}

pub fn has_usb_with_udid(udid: &str) -> bool {
    crate::watcher::CONNECTED_DEVICES
        .iter()
        .any(|d| d.as_usb().is_some_and(|u| u.udid() == udid))
}

pub fn has_network_with_udid(udid: &str) -> bool {
    crate::watcher::CONNECTED_DEVICES
        .iter()
        .any(|d| d.as_network().is_some_and(|u| u.udid == udid))
}

/// should this device be hidden from clients? (network shadowed by USB)
pub fn is_shadowed(dev: &Device) -> bool {
    match dev.as_network() {
        Some(ndev) => has_usb_with_udid(&ndev.udid),
        None => false,
    }
}

pub fn find_network_id_by_udid(udid: &str) -> Option<u64> {
    crate::watcher::CONNECTED_DEVICES
        .iter()
        .find(|dev| dev.as_network().is_some_and(|ndev| ndev.udid == udid))
        .map(|ndev| ndev.id())
}

pub async fn read_system_buid() -> Result<String, RusbmuxError> {
    let path = Path::new(LOCKDOWN_PATH).join("SystemConfiguration.plist");

    if !tokio::fs::try_exists(&path)
        .await
        .is_ok_and(|exists| exists)
    {
        let id = uuid::Uuid::new_v4().to_string().to_uppercase();
        let config = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
            "SystemBUID": &id
        }));
        tokio::fs::write(&path, config).await?;

        return Ok(id);
    }

    let config: plist::Value = plist::from_bytes(&tokio::fs::read(path).await?)?;
    config
        .as_dictionary()
        .ok_or(RusbmuxError::UnexpectedPacket(
            "Expected a packet with a dictionary plist payload".to_string(),
        ))?
        .get("SystemBUID")
        .ok_or(RusbmuxError::ValueNotFound(MissingFields::SystemBUID))?
        .as_string()
        .map(str::to_owned)
        .ok_or(RusbmuxError::InvalidData("SystemBUID is not a string"))
}
