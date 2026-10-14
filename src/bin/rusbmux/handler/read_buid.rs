use rusbmux::{
    error::RusbmuxError,
    usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxResult, UsbMuxVersion},
};

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

use crate::daemon::ConnectionStream;

pub async fn handle_read_buid(
    client: &mut ConnectionStream,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    let tag = usbmux_packet.header.tag;

    trace!(tag, "Reading SystemConfiguration.plist");
    let buid = match crate::utils::read_system_buid().await {
        Ok(buid) => buid,
        Err(RusbmuxError::IO(err)) => {
            error!(tag, %err, "Failed to read/write the SystemConfiguration.plist");
            let _ = super::send_result(client, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await;
            return Ok(());
        }
        Err(err) => return Err(err),
    };

    trace!(tag, buid, "Extracted SystemBUID");

    let response_plist = plist_macro::plist!({
        "BUID": &buid
    });

    let usbmux_packet = UsbMuxPacket::encode_from(
        plist_macro::plist_value_to_xml_bytes(&response_plist),
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        usbmux_packet.header.tag,
    );

    trace!(tag, "Sending BUID response");

    client.write_all(&usbmux_packet).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(tag, %err, "Failed to write BUID response")
        }
    })?;

    debug!(tag, "BUID response sent");

    Ok(())
}
