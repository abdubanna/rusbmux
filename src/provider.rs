use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{Buf, BytesMut};
use crossfire::TrySendError;
use idevice::{Idevice, IdeviceError, pairing_file::PairingFile, provider::IdeviceProvider};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    conn::{ConnState, UsbDeviceConn},
    device::{
        packet_router::SAsyncPacketRx,
        usb::{UsbDevice, UsbDeviceShared},
    },
    error::RusbmuxError,
    parser::device_mux::{TcpFlags, UsbDevicePacket},
};

/// a provider that exposes rusbmux's direct USB connection as an `idevice` provider
///
/// allows the `idevice` crate to connect to devices over USB without going
/// through a socket
#[derive(Debug)]
pub struct RusbmuxProvider {
    device: UsbDeviceShared,
    pairing_file: Option<PairingFile>,
    label: String,
    system_buid: String,
}

impl RusbmuxProvider {
    pub fn new(device: &UsbDevice, label: impl Into<String>, system_buid: Option<String>) -> Self {
        Self {
            device: device.shared(),
            pairing_file: None,
            system_buid: system_buid.unwrap_or(uuid::Uuid::new_v4().to_string().to_uppercase()),
            label: label.into(),
        }
    }

    pub fn new_from(
        device: UsbDeviceShared,
        label: impl Into<String>,
        system_buid: Option<String>,
    ) -> Self {
        Self {
            device,
            pairing_file: None,
            system_buid: system_buid.unwrap_or(uuid::Uuid::new_v4().to_string().to_uppercase()),
            label: label.into(),
        }
    }

    pub fn set_pairing_file(&mut self, pairing_file: PairingFile) {
        self.pairing_file = Some(pairing_file);
    }

    /// automatically checks the pairing file and generate a new one if needed
    ///
    /// returns true if it did generate a new one, false otherwise
    pub async fn preflight(&mut self) -> Result<bool, RusbmuxError> {
        if let Some(pairing_file) = crate::watcher::preflight(
            self.device.clone(),
            self.pairing_file.clone(),
            &self.system_buid,
        )
        .await?
        {
            self.set_pairing_file(pairing_file);
            return Ok(true);
        }

        Ok(false)
    }
}

impl IdeviceProvider for RusbmuxProvider {
    fn label(&self) -> &str {
        &self.label
    }

    fn get_pairing_file(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PairingFile, IdeviceError>> + Send>> {
        let pairing_file = self.pairing_file.clone();
        Box::pin(async move {
            if let Some(pairing_file) = pairing_file {
                Ok(pairing_file)
            } else {
                Err(IdeviceError::NotFound)
            }
        })
    }

    fn connect(
        &self,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Idevice, IdeviceError>> + Send>> {
        let label = self.label.clone();
        let device = self.device.clone();

        Box::pin(async move {
            let udid = device.udid().map(ToString::to_string);

            let (source_port, rx) = device.router.register(port).map_err(|err| {
                IdeviceError::UnexpectedResponse(format!(
                    "failed to connect to port {port} on {udid:?}: {err}"
                ))
            })?;

            tracing::debug!(
                device_id = device.id(),
                source_port,
                destination_port = port,
                "Creating new connection"
            );

            let tx = device.get_writer();

            let conn_state = ConnState::perform_handshake(source_port, port, &rx, &tx)
                .await
                .map_err(|err| {
                    IdeviceError::UnexpectedResponse(format!(
                        "failed to connect to port {port} on {udid:?}: {err}"
                    ))
                })?;

            let (_, dummy_rx) = crossfire::spsc::bounded_async(0);

            let conn = unsafe {
                UsbDeviceConn::new_from(
                    &device.core,
                    port,
                    source_port,
                    conn_state,
                    SAsyncPacketRx(dummy_rx),
                    tx,
                )
            };

            let stream = RusbmuxStream {
                device,
                read_buf: BytesMut::new(),
                need_ack: false,
                last_write_len: 0,
                read_stream: rx.0.into_stream(),
                write_sink: conn.get_tx().into_sink(),
                conn,
                write_packet: None,
            };

            let mut idevice = Idevice::new(Box::new(stream), label);

            if let Some(udid) = udid {
                idevice.set_udid(udid);
            }

            Ok(idevice)
        })
    }
}

/// bridges an rusbmux `UsbDeviceConn` to idevice's `ReadWrite` trait
///
/// mirrors the bidirectional pattern in daemon mode
///
/// incoming USB packets are buffered until consumed by the reader, while
/// outgoing writes are split according to the connection's current window size
struct RusbmuxStream {
    device: UsbDeviceShared,

    // the connection will shutdown on drop
    conn: UsbDeviceConn,

    read_buf: BytesMut,

    need_ack: bool,

    // the last sent bytes len, used to report back how much we've written
    last_write_len: usize,

    // these wrappers keep the registered waker across polls
    // tx.send()/rx.recv() creates a future, and dropping that future drops the waker
    read_stream: crossfire::stream::AsyncStream<crossfire::spsc::Array<UsbDevicePacket>>,
    write_sink: crossfire::sink::AsyncSink<crossfire::mpsc::Array<UsbDevicePacket>>,

    // the packet to be sent
    write_packet: Option<UsbDevicePacket>,
}

// SAFETY: because of the `Cell` inside of stream and sink, which is comming from the `AsyncRx/Tx`, which has a
// phantom `Cell` to !Sync it
//
// the Sync varient of it (MAsyncRx/Tx) is a wrapper, that does an `unsafe impl Sync` to it
//
// so I think this is fine
unsafe impl Sync for RusbmuxStream {}

impl std::fmt::Debug for RusbmuxStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RusbmuxStream")
            .field("device", &self.device)
            .field("conn", &self.conn)
            .field("read_buf_len", &self.read_buf.len())
            .field("need_ack", &self.need_ack)
            .field("last_write_len", &self.last_write_len)
            .field("has_pending_write", &self.write_packet.is_some())
            .field(
                "channels",
                &format_args!(
                    "rx_disconnected={}, tx_disconnected={}",
                    self.read_stream.is_disconnected(),
                    self.write_sink.is_disconnected(),
                ),
            )
            .finish()
    }
}

impl RusbmuxStream {
    fn poll_send_flag(
        &mut self,
        cx: &mut Context<'_>,
        tcp_flag: TcpFlags,
    ) -> Poll<std::io::Result<()>> {
        match self
            .write_sink
            .poll_send(cx, self.conn.build_flag(tcp_flag))
        {
            Ok(()) => {
                self.conn.update_sendable_bytes();
                self.need_ack = false;
                Poll::Ready(Ok(()))
            }
            Err(TrySendError::Full(_)) => Poll::Pending,
            Err(TrySendError::Disconnected(_p)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "Channel disconnected",
            ))),
        }
    }

    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.read_stream.poll_item(cx) {
            Poll::Ready(Some(packet)) => {
                self.conn.update_states(&packet);
                let payload = packet.payload.encode();

                // if it's empty, then it's zero-copy
                if self.read_buf.is_empty() {
                    self.read_buf = BytesMut::from(payload);
                } else {
                    self.read_buf.extend_from_slice(&payload);
                }

                self.need_ack = true;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "Channel disconnected",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_send_pending(
        &mut self,
        cx: &mut Context<'_>,
        packet: UsbDevicePacket,
    ) -> Poll<std::io::Result<usize>> {
        let payload_len = packet.payload.len() as u32;
        match self.write_sink.poll_send(cx, packet) {
            Ok(()) => {
                self.conn.add_sent_bytes(payload_len);
                self.conn.update_sendable_bytes();
                let n = self.last_write_len;
                self.last_write_len = 0;
                Poll::Ready(Ok(n))
            }
            Err(TrySendError::Full(p)) => {
                self.write_packet = Some(p);
                Poll::Pending
            }
            Err(TrySendError::Disconnected(_p)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "Channel disconnected",
            ))),
        }
    }
}

impl AsyncRead for RusbmuxStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if self.need_ack {
                std::task::ready!(self.poll_send_flag(cx, TcpFlags::ACK))?;
            } else if !self.read_buf.is_empty() {
                let n = self.read_buf.len().min(buf.remaining());
                buf.put_slice(&self.read_buf[..n]);
                self.read_buf.advance(n);
                if buf.remaining() != 0 && !self.read_buf.is_empty() {
                    continue;
                }
                return Poll::Ready(Ok(()));
            } else {
                // NOTE: could a device return only empty packets?
                std::task::ready!(self.poll_recv(cx))?;
            }
        }
    }
}

impl AsyncWrite for RusbmuxStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        loop {
            if self.need_ack {
                std::task::ready!(self.poll_send_flag(cx, TcpFlags::ACK))?;
            }

            if let Some(packet) = self.write_packet.take() {
                let n = std::task::ready!(self.poll_send_pending(cx, packet))?;
                return Poll::Ready(Ok(n));
            } else {
                let sendable = self.conn.get_sendable_bytes();

                if sendable == 0 {
                    // read to widen the window
                    std::task::ready!(self.poll_recv(cx))?;
                    continue;
                }

                let n = buf.len().min(sendable);
                self.last_write_len = n;

                self.write_packet = Some(self.conn.build_bytes(BytesMut::from(&buf[..n]).freeze()));
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if !self.conn.dropped() {
            std::task::ready!(self.poll_send_flag(cx, TcpFlags::RST))?;

            unsafe { self.conn.set_dropped() };
        }

        Poll::Ready(Ok(()))
    }
}
