use std::path::Path;

use crate::{
    AsyncWriting,
    error::{MissingFields, RusbmuxError},
    handler::{LOCKDOWN_PATH, send_result},
    parser::usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxResult, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub(crate) async fn read_system_buid() -> Result<String, RusbmuxError> {
    let path = Path::new(LOCKDOWN_PATH).join("SystemConfiguration.plist");

    if tokio::fs::try_exists(&path)
        .await
        .is_ok_and(|exists| exists)
    {
        let id = uuid::Uuid::new_v4().to_string().to_uppercase();
        let config = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
            "SystemBUID": id
        }));
        tokio::fs::write(&path, config).await?;
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

pub async fn handle_read_buid(
    writer: &mut impl AsyncWriting,
    usbmux_packet: &UsbMuxPacket,
) -> Result<(), RusbmuxError> {
    let tag = usbmux_packet.header.tag;

    trace!(tag, "Reading SystemConfiguration.plist");
    let buid = match read_system_buid().await {
        Ok(buid) => buid,
        Err(RusbmuxError::IO(err)) => {
            error!(tag, %err, "Failed to write a new SystemConfiguration.plist");
            let _ = send_result(writer, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await;
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

    writer.write_all(&usbmux_packet).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(tag, %err, "Failed to write BUID response")
        }
    })?;

    debug!(tag, "BUID response sent");

    Ok(())
}
