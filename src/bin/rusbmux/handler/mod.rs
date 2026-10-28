use std::{io::ErrorKind, ops::ControlFlow};

use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, trace, warn};

use rusbmux::{
    error::{MissingFields, ParseError, RusbmuxError},
    usbmux::{UsbMuxMsgType, UsbMuxPacket, UsbMuxRequest, UsbMuxResult, UsbMuxVersion},
};

use crate::daemon::ConnectionStream;

mod add_device;
mod connect;
mod delete_pair_record;
mod device_list;
mod listen;
mod listeners_list;
mod read_buid;
mod read_pair_record;
mod save_pair_record;

#[cfg(target_os = "macos")]
pub const LOCKDOWN_PATH: &str = "/var/db/lockdown";

#[cfg(target_os = "linux")]
pub const LOCKDOWN_PATH: &str = "/var/lib/lockdown";

#[cfg(windows)]
pub const LOCKDOWN_PATH: &str = "C:\\ProgramData\\Apple\\Lockdown";

pub async fn handle_client(mut client: ConnectionStream) {
    loop {
        let usbmux_packet = match UsbMuxPacket::from_reader(&mut client).await {
            Ok(p) => p,

            // client closed connection
            Err(ParseError::IO(err))
                if matches!(
                    err.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
                ) =>
            {
                warn!("Client socket disconnected, closing");
                break;
            }

            Err(err) => {
                error!(%err, "Failed to read usbmux packet");
                continue;
            }
        };

        let tag = usbmux_packet.header.tag;

        debug!(
            tag,
            msg_type = ?usbmux_packet.header.msg_type,
            "Received usbmux packet"
        );

        match handle_message(&mut client, usbmux_packet).await {
            // comes from the ones that transforms the connection (Connect, Listen), because you're
            // not supposed to do anything else if those failed
            Ok(ControlFlow::Break(())) => {
                return;
            }

            Err(HandlerError {
                err,
                request,
                fatal,
            }) if fatal => {
                if crate::utils::is_disconnect(&err) {
                    debug!(
                        tag,
                        ?request,
                        %err,
                        "Client disconnected while processing request, bye"
                    );
                    return;
                }

                error!(tag, ?request, %err, "Handler failed, bye");
                return;
            }

            Err(HandlerError { err, request, .. }) => {
                // if the client disconnected, then there's no reason to continue
                if crate::utils::is_disconnect(&err) {
                    debug!(
                        tag,
                        ?request,
                        %err,
                        "Client disconnected while processing request, bye"
                    );
                    return;
                }

                // it's an error, but that doesn't mean to close the connection
                error!(tag, ?request, %err, "Handler failed");
                continue;
            }

            Ok(ControlFlow::Continue(())) => continue,
        }
    }
}

async fn handle_message(
    client: &mut ConnectionStream,
    usbmux_packet: UsbMuxPacket,
) -> Result<ControlFlow<()>, HandlerError> {
    let tag = usbmux_packet.header.tag;

    match usbmux_packet.header.msg_type {
        UsbMuxMsgType::MessagePlist => {
            // TODO: implement binary payload

            let Some(payload) = usbmux_packet.payload.as_plist() else {
                send_result(client, UsbMuxResult::BadCommand, usbmux_packet.header.tag).await?;
                return Err(
                    RusbmuxError::UnexpectedPacket("expected plist payload".to_string()).into(),
                );
            };

            debug!(
                "Received payload: {}",
                plist_macro::pretty_print_plist(payload)
            );

            let usbmux_request: Result<UsbMuxRequest, RusbmuxError> = plist::from_value(payload)
                .map_err(|err| {
                    let err_str = err.to_string();

                    for field in [
                        MissingFields::PairRecordID,
                        MissingFields::PairRecordData,
                        MissingFields::DeviceID,
                        MissingFields::PortNumber,
                    ] {
                        if err_str.contains(&format!("missing field `{field:?}`")) {
                            return RusbmuxError::ValueNotFound(field);
                        }
                    }

                    RusbmuxError::Parse(ParseError::Plist(err))
                });

            let usbmux_request = match usbmux_request {
                Ok(r) => r,
                Err(err) => {
                    let code = match &err {
                        RusbmuxError::ValueNotFound(field) => field.result_code(),
                        _ => UsbMuxResult::InvalidInput,
                    };

                    send_result(client, code, tag).await?;

                    return Err(err.into());
                }
            };

            match usbmux_request {
                UsbMuxRequest::ListDevices { .. } => {
                    device_list::handle_device_list(client, usbmux_packet.header.tag)
                        .await
                        .map_err(|err| (err, "ListDevices"))?;
                }

                UsbMuxRequest::Listen { .. } => {
                    info!(tag, "Client entered listen mode");
                    listen::handle_listen(client, usbmux_packet.header.tag)
                        .await
                        .map_err(|err| (err, "Listen", true))?;

                    info!(tag, "Listener handed off");
                    return Ok(ControlFlow::Break(()));
                }
                UsbMuxRequest::ListListeners { .. } => {
                    listeners_list::handle_listeners_list(client, usbmux_packet.header.tag)
                        .await
                        .map_err(|err| (err, "ListListeners"))?;
                }
                UsbMuxRequest::ReadPairRecord { pair_record_id, .. } => {
                    read_pair_record::handle_read_pair_record(
                        client,
                        pair_record_id,
                        usbmux_packet.header.tag,
                    )
                    .await
                    .map_err(|err| (err, "ReadPairRecord"))?;
                }
                UsbMuxRequest::Connect {
                    device_id, port, ..
                } => {
                    info!(tag, "Client entered connect mode");

                    connect::handle_connect(client, device_id, port, usbmux_packet.header.tag)
                        .await
                        .map_err(|err| (err, "Connect", true))?;

                    info!(tag, "Connection handed off");
                    return Ok(ControlFlow::Break(()));
                }
                UsbMuxRequest::ReadBUID { .. } => {
                    read_buid::handle_read_buid(client, &usbmux_packet)
                        .await
                        .map_err(|err| (err, "ReadBUID"))?;
                }
                UsbMuxRequest::SavePairRecord {
                    pair_record_id,
                    pair_record_data,
                    device_id,
                    ..
                } => {
                    save_pair_record::handle_save_pair_record(
                        client,
                        pair_record_id,
                        pair_record_data,
                        device_id,
                        usbmux_packet.header.tag,
                    )
                    .await
                    .map_err(|err| (err, "SavePairRecord"))?;
                }
                UsbMuxRequest::DeletePairRecord { pair_record_id, .. } => {
                    delete_pair_record::handle_delete_pair_record(client, pair_record_id, tag)
                        .await
                        .map_err(|err| (err, "DeletePairRecord"))?;
                }
                UsbMuxRequest::AddDevice {
                    ip, udid, force, ..
                } => {
                    add_device::handle_add_device(client, ip, udid, force, tag)
                        .await
                        .map_err(|err| (err, "AddDevice"))?;
                }
            }
        }
        // TODO: are others necessary?
        _ => send_result(client, UsbMuxResult::BadCommand, usbmux_packet.header.tag).await?,
    }

    Ok(ControlFlow::Continue(()))
}

pub async fn send_result(
    client: &mut ConnectionStream,
    code: UsbMuxResult,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let result_payload = plist_macro::plist!({
        "MessageType": "Result",
        "Number": (code as u16)
    });

    let result_payload_xml = plist_macro::plist_value_to_xml_bytes(&result_payload);

    let result_usbmux_packet = UsbMuxPacket::encode_from(
        result_payload_xml,
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        tag,
    );
    client
        .write_all(&result_usbmux_packet)
        .await
        .inspect_err(|err| {
            if !crate::utils::is_disconnect_io(err) {
                error!(tag, %err, "Failed to send OKAY")
            }
        })?;

    trace!(tag, "Sent OKAY response");

    Ok(())
}

fn check_pair_record_id(pair_record_id: &str) -> Result<(), RusbmuxError> {
    if pair_record_id.contains('/')
        || pair_record_id.contains('\\')
        || pair_record_id.contains("..")
    {
        warn!(?pair_record_id, "malicious pair record id detected");
        return Err(RusbmuxError::UnexpectedPacket(
            "Given pair record id is malformed".into(),
        ));
    }

    Ok(())
}

struct HandlerError {
    err: RusbmuxError,
    request: Option<&'static str>,
    fatal: bool,
}

impl From<(RusbmuxError, &'static str, bool)> for HandlerError {
    fn from(value: (RusbmuxError, &'static str, bool)) -> Self {
        Self {
            err: value.0,
            request: Some(value.1),
            fatal: value.2,
        }
    }
}

impl From<(RusbmuxError, &'static str)> for HandlerError {
    fn from(value: (RusbmuxError, &'static str)) -> Self {
        Self {
            err: value.0,
            request: Some(value.1),
            fatal: false,
        }
    }
}

impl From<RusbmuxError> for HandlerError {
    fn from(value: RusbmuxError) -> Self {
        Self {
            err: value,
            request: None,
            fatal: false,
        }
    }
}
