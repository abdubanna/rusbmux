use std::{io::ErrorKind, path::Path};

use crate::daemon::ConnectionStream;

use super::{LOCKDOWN_PATH, send_result};
use rusbmux::{
    error::RusbmuxError,
    usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxResult, UsbMuxVersion},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

pub async fn handle_read_pair_record(
    client: &mut ConnectionStream,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    if let Err(err) = read_pair_record(client, pair_record_id, tag).await {
        match err {
            RusbmuxError::UnexpectedPacket(_) => {
                send_result(client, UsbMuxResult::BadCommand, tag).await?;
            }
            RusbmuxError::IO(ref io_err)
                if matches!(
                    io_err.kind(),
                    ErrorKind::PermissionDenied | ErrorKind::NotFound
                ) =>
            {
                send_result(client, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await?;
            }
            _ => {}
        }

        return Err(err);
    }

    Ok(())
}

pub async fn read_pair_record(
    client: &mut ConnectionStream,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    trace!(tag, pair_record_id, "Reading pair record");

    super::check_pair_record_id(&pair_record_id)?;

    let path = Path::new(LOCKDOWN_PATH).join(format!("{pair_record_id}.plist"));

    trace!(tag, ?path, "Reading pairing file");

    let pairing_file = tokio::fs::read(&path).await.inspect_err(|err| {
        error!(
            tag,
            pair_record_id,
            ?path,
            %err,
            "Failed to read pairing file"
        );
    })?;

    trace!(
        tag,
        pair_record_id,
        size = pairing_file.len(),
        "Pairing file loaded"
    );

    let pairing_file_xml = plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
        "PairRecordData": pairing_file
    }));

    let usbmux_packet = UsbMuxPacket::encode_from(
        pairing_file_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );

    trace!(tag, "Sending pair record response");

    client.write_all(&usbmux_packet).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(
                tag,
                pair_record_id,
                %err,
                "Failed to write read pair record response"
            );
        }
    })?;

    debug!(tag, pair_record_id, "Pair record sent");

    Ok(())
}
