use std::{io::ErrorKind, path::Path};

use tracing::{debug, error};

use rusbmux::{error::RusbmuxError, usbmux::UsbMuxResult};

use crate::daemon::ConnectionStream;

use super::{LOCKDOWN_PATH, send_result};

pub async fn handle_delete_pair_record(
    client: &mut ConnectionStream,
    pair_record_id: String,
    tag: u32,
) -> Result<(), RusbmuxError> {
    match delete_pair_record(pair_record_id, tag).await {
        Ok(()) => send_result(client, UsbMuxResult::OK, tag).await?,
        Err(err) => {
            match err {
                RusbmuxError::UnexpectedPacket(_) => {
                    send_result(client, UsbMuxResult::BadCommand, tag).await?;
                }

                RusbmuxError::IO(ref io_err) if io_err.kind() == ErrorKind::NotFound => {
                    send_result(client, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await?;
                }
                _ => {}
            }
            return Err(err);
        }
    }

    Ok(())
}

pub async fn delete_pair_record(pair_record_id: String, tag: u32) -> Result<(), RusbmuxError> {
    debug!(tag, pair_record_id, "Deleting pair record");

    super::check_pair_record_id(&pair_record_id)?;

    let path = Path::new(LOCKDOWN_PATH).join(format!("{pair_record_id}.plist"));

    tokio::fs::remove_file(&path).await.inspect_err(
        |err| error!(tag, pair_record_id, ?path, %err, "Failed to delete pair record"),
    )?;

    debug!(tag, pair_record_id, ?path, "Pair record deleted");

    Ok(())
}
