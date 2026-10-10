use crate::{
    AsyncWriting,
    error::RusbmuxError,
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxVersion},
    watcher::HOTPLUG_EVENT_TX,
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_listeners_list(
    writer: &mut impl AsyncWriting,
    tag: u32,
) -> Result<(), RusbmuxError> {
    // TODO: use get_hotplug_event_tx()
    let event_tx = HOTPLUG_EVENT_TX
        .get()
        .ok_or(RusbmuxError::HotPlugNotSupported)?;

    let mut listeners_plist = vec![];
    for _ in 0..event_tx.receiver_count() {
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
    writer.write_all(&usbmux_packet).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(
                tag,
                %err,
                "Failed to write listeners list packet"
            );
        }
    })?;

    debug!(
        tag,
        listeners = event_tx.receiver_count(),
        "Listeners list sent"
    );

    Ok(())
}
