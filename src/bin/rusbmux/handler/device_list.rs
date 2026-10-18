use rusbmux::{
    error::RusbmuxError,
    usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

use crate::{daemon::ConnectionStream, watcher::CONNECTED_DEVICES};

use tokio::io::AsyncWriteExt;
use tracing::{debug, error};

pub async fn devices_plist() -> Result<plist::Value, RusbmuxError> {
    let mut devices_plist = Vec::with_capacity(CONNECTED_DEVICES.len());

    // prefer USB if the device is connected on both USB and WiFi
    for device in CONNECTED_DEVICES
        .iter()
        .filter(|dev| !crate::utils::is_shadowed(dev))
    {
        devices_plist.push(device.create_device_attached());
    }

    debug!(
        "Created device list plist with {} device/s",
        devices_plist.len()
    );

    let res = plist_macro::plist!({
        "DeviceList": devices_plist
    });

    debug!("{}", plist_macro::pretty_print_plist(&res));

    Ok(res)
}

pub async fn handle_device_list(
    client: &mut ConnectionStream,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let devices_plist = devices_plist().await?;

    let devices_xml = plist_macro::plist_value_to_xml_bytes(&devices_plist);

    let usbmux_packet = UsbMuxPacket::encode_from(
        devices_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );
    client.write_all(&usbmux_packet).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(tag, %err, "Failed to send device list packet")
        }
    })?;

    debug!(tag, "Device list packet sent");

    Ok(())
}
