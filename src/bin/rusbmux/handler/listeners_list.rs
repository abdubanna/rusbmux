use rusbmux::{
    error::RusbmuxError,
    usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
};

use crate::{daemon::ConnectionStream, watcher::get_hotplug_event_tx};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_listeners_list(
    client: &mut ConnectionStream,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let listeners_count = get_hotplug_event_tx().await.receiver_count();
    let mut listeners_plist = Vec::with_capacity(listeners_count);

    for _ in 0..listeners_count {
        listeners_plist.push(plist_macro::plist!({
            "Blacklisted": false,
            "ConnType": 0,

            // TODO:
            "ID String": "unknown",
            "ProgName": "unknown",
        }));
    }

    let listeners_plist_result = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
        "ListenerList": listeners_plist
    }));

    let usbmux_packet = UsbMuxPacket::encode_from(
        listeners_plist_result,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );

    trace!(tag, "Sending listeners list response");
    client.write_all(&usbmux_packet).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(
                tag,
                %err,
                "Failed to write listeners list packet"
            );
        }
    })?;

    debug!(tag, listeners_count, "Listeners list sent");

    Ok(())
}
