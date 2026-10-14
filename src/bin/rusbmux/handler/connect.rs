use std::sync::Arc;

use rusbmux::{
    conn::{DeviceConn, NetworkDeviceConn, UsbDeviceConn},
    error::RusbmuxError,
    usbmux::UsbMuxResult,
};

use crate::{daemon::ConnectionStream, watcher::CONNECTED_DEVICES};

use super::send_result;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info, trace};

#[cfg(unix)]
pub type ReadHalf<'a> = tokio::net::unix::ReadHalf<'a>;
#[cfg(windows)]
pub type ReadHalf<'a> = tokio::net::tcp::ReadHalf<'a>;

#[cfg(unix)]
pub type WriteHalf<'a> = tokio::net::unix::WriteHalf<'a>;
#[cfg(windows)]
pub type WriteHalf<'a> = tokio::net::tcp::WriteHalf<'a>;

const CLIENT_BUFF_SIZE: usize = 128 * 1024;

pub async fn handle_connect(
    client: &mut ConnectionStream,
    device_id: u64,
    destination_port: u16,
    tag: u32,
) -> Result<(), RusbmuxError> {
    let conn = match connect(device_id, destination_port, tag).await {
        Ok(c) => c,
        Err(err) => {
            match &err {
                RusbmuxError::DeviceNotFound(_) | RusbmuxError::RanOutOfSourcePort => {
                    send_result(client, UsbMuxResult::BadDeviceOrNoSuchFile, tag).await?;
                }

                _ => {
                    send_result(client, UsbMuxResult::ConnectionRefused, tag).await?;
                }
            }
            return Err(err);
        }
    };

    send_result(client, UsbMuxResult::OK, tag).await?;

    match conn {
        DeviceConn::Usb(conn) => handle_usb_device_connect(client, conn).await?,
        DeviceConn::Network(conn) => handle_network_device_connect(client, conn).await?,
    }

    Ok(())
}

pub async fn handle_network_device_connect(
    client: &mut ConnectionStream,
    mut conn: NetworkDeviceConn,
) -> Result<(), RusbmuxError> {
    let device_id = conn.device_id();
    let destination_port = conn.destination_port();

    let canceler = conn.device_canceler();

    tokio::select! {
        res = tokio::io::copy_bidirectional_with_sizes(
            &mut conn.stream,
            client,
            CLIENT_BUFF_SIZE,
            CLIENT_BUFF_SIZE
        ) => {
            res?;
            Ok(())
        }

        _ = canceler.cancelled() => {
            debug!(device_id, destination_port, "Shutting down connection");
            Ok(())
        }
    }
}

pub async fn handle_usb_device_connect(
    client: &mut ConnectionStream,
    conn: Arc<UsbDeviceConn>,
) -> Result<(), RusbmuxError> {
    let device_id = conn.device_id();
    let destination_port = conn.destination_port();

    let mut read_buf = BytesMut::with_capacity(CLIENT_BUFF_SIZE);
    let (mut client_reader, mut client_writer) = client.split();

    loop {
        tokio::select! {
            _ = conn.wait_shutdown() => {
                debug!(device_id, destination_port, "Device is shutting down");
                return Ok(());
            }

            packet = conn.recv() => {
                let packet = packet?;
                debug!(device_id, destination_port, "Received packet from device");

                client_send(&mut client_writer, packet.payload.encode()).await?;
            }

            client_packet = client_read(&mut client_reader, &mut read_buf, conn.get_sendable_bytes()),
                            if conn.get_sendable_bytes() > 0
            => {
                let client_packet = client_packet?;

                if client_packet.is_empty() {
                    info!(device_id, destination_port, "Client disconnected");
                    conn.close().await?;
                    return Ok(());
                }

                debug!(device_id, destination_port, "Processing client packet");

                conn.send_bytes(client_packet.freeze()).await?;
            }
        };
    }
}

pub async fn client_read(
    client: &mut ReadHalf<'_>,
    buf: &mut BytesMut,
    sendable_bytes: usize,
) -> Result<BytesMut, RusbmuxError> {
    if !buf.is_empty() {
        return Ok(buf.split_to(sendable_bytes.min(buf.len())));
    }

    if !buf.try_reclaim(CLIENT_BUFF_SIZE) || buf.capacity() == 0 {
        buf.reserve(sendable_bytes);
    }

    client.read_buf(buf).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(%err, "Failed to read from client");
        }
    })?;

    Ok(buf.split_to(sendable_bytes.min(buf.len())))
}

pub async fn client_send(client: &mut WriteHalf<'_>, payload: Bytes) -> Result<(), RusbmuxError> {
    trace!(len = payload.len(), "Sending packet to client");

    client.write_all(&payload).await.inspect_err(|err| {
        if !crate::utils::is_disconnect_io(err) {
            error!(%err, "Failed to write packet to client")
        }
    })?;

    Ok(())
}

pub async fn connect(
    device_id: u64,
    destination_port: u16,
    tag: u32,
) -> Result<DeviceConn, RusbmuxError> {
    info!(device_id, destination_port, tag, "Client connecting");

    let device = CONNECTED_DEVICES
        .get(&device_id)
        .ok_or(RusbmuxError::DeviceNotFound(device_id))?;

    let conn = device.connect(destination_port).await?;

    Ok(conn)
}
