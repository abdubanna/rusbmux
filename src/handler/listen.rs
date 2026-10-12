use crate::{
    AsyncWriting,
    error::RusbmuxError,
    handler::send_result,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxResult, UsbMuxVersion},
    watcher::{CONNECTED_DEVICES, DeviceEvent, HOTPLUG_EVENT_TX},
};

use tokio::{io::AsyncWriteExt, sync::broadcast};
use tracing::{debug, error, info, trace, warn};

pub async fn handle_listen(writer: &mut impl AsyncWriting, tag: u32) -> Result<(), RusbmuxError> {
    // TODO: use get_hotplug_event_tx()
    let mut event_receiver = match HOTPLUG_EVENT_TX
        .get()
        .ok_or(RusbmuxError::HotPlugNotSupported)
        .map(broadcast::Sender::subscribe)
    {
        Ok(r) => r,
        Err(err) => {
            send_result(writer, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await?;
            return Err(err);
        }
    };

    send_result(writer, UsbMuxResult::OK, tag).await?;

    send_currently_connected(writer, tag).await?;

    debug!(tag, "Listening for device attach/detach events");

    while let Ok(event) = event_receiver
        .recv()
        .await
        .inspect_err(|err| warn!(%err, "Failed to receive hotplug events"))
    {
        match event {
            DeviceEvent::Attached { id } => {
                info!(id, "Device attached");
                let Some(device) = CONNECTED_DEVICES.get(&id) else {
                    warn!(
                        id,
                        "Device disappeared before attach event could be processed"
                    );
                    continue;
                };

                let device_plist = device.create_device_attached()?;

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let connected_packet = UsbMuxPacket::encode_from(
                    device_xml,
                    UsbMuxVersion::Plist,
                    UsbMuxMsgType::MessagePlist,
                    tag,
                );
                writer
                    .write_all(&connected_packet)
                    .await
                    .inspect_err(|err| {
                        if !crate::utils::is_disconnect_io(err) {
                            error!(device_id = id, tag, %err, "Failed to send device attach event")
                        }
                    })?;

                trace!(device_id = id, tag, "Attach event sent");
            }
            DeviceEvent::Detached { id } => {
                info!(id, "Device detached");

                let device_plist = plist_macro::plist!({
                    "MessageType": "Detached",
                    "DeviceID": id
                });

                let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

                let disconnected_packet = UsbMuxPacket::encode_from(
                    device_xml,
                    UsbMuxVersion::Plist,
                    UsbMuxMsgType::MessagePlist,
                    tag,
                );
                writer
                    .write_all(&disconnected_packet)
                    .await
                    .inspect_err(|err| {
                        if !crate::utils::is_disconnect_io(err) {
                            error!(device_id = id, tag, %err, "Failed to send device detach event")
                        }
                    })?;

                trace!(device_id = id, tag, "Detach event sent");
            }
        }
    }

    warn!(tag, "Device listen session ended");

    Ok(())
}

pub async fn send_currently_connected(
    writer: &mut impl AsyncWriting,
    tag: u32,
) -> Result<(), RusbmuxError> {
    // TODO: put it in a function
    for device in CONNECTED_DEVICES
        .iter()
        .filter(|dev| !crate::utils::is_shadowed(dev))
    {
        let device_plist = device.create_device_attached()?;

        let device_xml = plist_macro::plist_value_to_xml_bytes(&device_plist);

        let connected_packet = UsbMuxPacket::encode_from(
            device_xml,
            UsbMuxVersion::Plist,
            UsbMuxMsgType::MessagePlist,
            tag,
        );
        writer.write_all(&connected_packet).await.inspect_err(|err|
            if !crate::utils::is_disconnect_io(err) {
                error!(device_id = device.id(), tag, %err, "Failed to send initial device packet")
            }
        )?;
    }

    Ok(())
}
