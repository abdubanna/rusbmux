use std::{io::ErrorKind, path::Path};

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, trace};

use crate::daemon::ConnectionStream;

use super::{LOCKDOWN_PATH, send_result};
use rusbmux::{
    error::RusbmuxError,
    usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxResult, UsbMuxVersion},
};

pub async fn handle_save_pair_record(
    client: &mut ConnectionStream,
    pair_record_id: String,
    pair_record_data: plist::Data,
    device_id: Option<u64>,
    tag: u32,
) -> Result<(), RusbmuxError> {
    match save_pair_record(client, pair_record_id, pair_record_data, device_id, tag).await {
        Ok(()) => {
            send_result(client, UsbMuxResult::OK, tag).await?;
        }

        Err(err) => {
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
    }

    Ok(())
}

pub async fn save_pair_record(
    client: &mut ConnectionStream,
    pair_record_id: String,
    pair_record_data: plist::Data,
    device_id: Option<u64>,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let pair_record_data: Vec<u8> = pair_record_data.into();
    trace!(
        tag,
        pair_record_id,
        data_len = pair_record_data.len(),
        "Received pair record data"
    );

    super::check_pair_record_id(&pair_record_id)?;

    let path = Path::new(LOCKDOWN_PATH).join(format!("{pair_record_id}.plist"));

    trace!(tag, pair_record_id, ?path, "Writing pair record to disk");

    // TODO: permissions
    tokio::fs::write(&path, pair_record_data)
        .await
        .inspect_err(|err| {
            error!(
                tag,
                pair_record_id,
                ?path,
                %err,
                "Failed to write pair record file"
            )
        })?;

    debug!(tag, pair_record_id, "Pair record saved");

    // send a paired message if the `DeviceID` is provided, it's not necessary, but it's there for
    // backword compatibility
    if let Some(device_id) = device_id {
        trace!(tag, pair_record_id, device_id, "Sending paired message");

        let pair_response = UsbMuxPacket::encode_from(
            plist_macro::plist_value_to_xml_bytes(&plist_macro::plist!({
                "MessageType": "Paired",
                "DeviceID": device_id
            })),
            UsbMuxVersion::Plist,
            UsbMuxMsgType::MessagePlist,
            tag,
        );

        client.write_all(&pair_response).await.inspect_err(|err| {
            if !crate::utils::is_disconnect_io(err) {
                error!(
                    tag,
                    pair_record_id,
                    %err,
                    "Failed to send paired response"
                );
            }
        })?;

        trace!(tag, pair_record_id, "Paired response sent");
    } else {
        trace!(tag, "DeviceID is not provided, skipping the paired message");
    }

    Ok(())
}
