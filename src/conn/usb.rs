use bytes::Bytes;
use crossfire::{MAsyncTx, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use crate::{
    device::{WriterMsg, core::DeviceCore, packet_router::SAsyncPacketRx, usb::UsbDevice},
    error::RusbmuxError,
    parser::device_mux::{AUTO_SEQ, TcpFlags, UsbDevicePacket},
    usb_backend::MAX_PACKET_PAYLOAD_SIZE,
};

#[derive(Debug)]
pub struct ConnState {
    pub sent_bytes: u32,
    pub received_bytes: u32,
    pub device_window_size: u16,
    pub device_received_bytes: u32,
}

impl ConnState {
    pub async fn perform_handshake(
        source_port: u16,
        destination_port: u16,
        rx: &SAsyncPacketRx,
        tx: &MAsyncTx<mpsc::Array<WriterMsg>>,
    ) -> Result<Self, RusbmuxError> {
        let mut sent_bytes = 0;
        let mut received_bytes = 0;

        info!(source_port, destination_port, "Initiating TCP handshake");

        let tcp_syn = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                source_port,
                destination_port,
                sent_bytes,
                received_bytes,
                TcpFlags::SYN,
            )
            .build();

        // let tcp_syn_header = tcp_syn.header;

        tx.send(WriterMsg::Packet(tcp_syn)).await?;
        trace!(source_port, destination_port, "Sent SYN");

        let tcp_syn_ack = rx.recv().await?;

        let tcp_syn_ack_tcp_hdr =
            tcp_syn_ack
                .tcp_hdr
                .as_ref()
                .ok_or(RusbmuxError::UnexpectedPacket(
                    "Expected a packet with a tcp header".to_string(),
                ))?;

        if !(tcp_syn_ack_tcp_hdr.syn && tcp_syn_ack_tcp_hdr.ack) {
            tracing::warn!(
                tcp_hdr = ?tcp_syn_ack_tcp_hdr,
                payload = ?tcp_syn_ack.payload.encode(),
                "Received a non SYN-ACK packet"
            );

            // the reader loop would remove the connection from the router
            return Err(RusbmuxError::IO(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "Got a non SYN-ACK",
            )));
        }

        debug!(source_port, destination_port, "Received SYN-ACK");

        // let our_send_seq = tcp_syn_header.as_v2().unwrap().send_seq.get();
        // let device_recv_seq = tcp_syn_ack.header.as_v2().map(|h| h.recv_seq.get());
        //
        // debug_assert!(
        //     device_recv_seq.is_some_and(|seq| seq < our_send_seq),
        //     "device is behind or out-of-order"
        // );

        // should be 1 (syn)
        sent_bytes += tcp_syn_ack_tcp_hdr.acknowledgment_number;

        // I've received 1 byte (syn-ack)
        received_bytes += 1;

        let tcp_ack = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                source_port,
                destination_port,
                sent_bytes,
                received_bytes,
                TcpFlags::ACK,
            )
            .build();

        tx.send(WriterMsg::Packet(tcp_ack)).await?;

        trace!(source_port, destination_port, "Sent ACK");

        info!(
            source_port,
            destination_port, sent_bytes, received_bytes, "TCP handshake complete"
        );

        Ok(Self {
            sent_bytes,
            received_bytes,
            device_window_size: tcp_syn_ack_tcp_hdr.window_size,
            device_received_bytes: tcp_syn_ack_tcp_hdr.acknowledgment_number,
        })
    }

    pub fn calc_sendable_bytes(&self) -> usize {
        // the device right shifts the window size (so are we), so put it back to get the actual
        // value
        let device_window_size = (self.device_window_size as u32) << 8;

        let unacked_bytes = self.sent_bytes.wrapping_sub(self.device_received_bytes);

        if device_window_size > unacked_bytes {
            ((device_window_size - unacked_bytes) as usize).min(MAX_PACKET_PAYLOAD_SIZE)
        } else {
            0
        }
    }
}

#[derive(Debug)]
pub struct UsbDeviceConn {
    device_core: DeviceCore,
    source_port: u16,
    destination_port: u16,

    sendable_bytes: usize,

    conn_state: ConnState,
    rx: SAsyncPacketRx,
    tx: MAsyncTx<mpsc::Array<WriterMsg>>,

    dropped: bool,
}

impl UsbDeviceConn {
    pub(crate) const WINDOW_SIZE: u16 = ((128u32 * 1024) >> 8) as u16;

    /// # Safety
    ///
    /// make sure the connection is already opened and you took the values from the exact previous
    /// connection
    pub(crate) unsafe fn new_from(
        device_core: &DeviceCore,
        destination_port: u16,
        source_port: u16,
        conn_state: ConnState,
        rx: SAsyncPacketRx,
        tx: MAsyncTx<mpsc::Array<WriterMsg>>,
    ) -> Self {
        let sendable_bytes = conn_state.calc_sendable_bytes();

        Self {
            device_core: device_core.clone(),
            source_port,
            destination_port,
            sendable_bytes,
            conn_state,
            rx,
            tx,
            dropped: false,
        }
    }

    pub(crate) async fn new(
        device: &UsbDevice,
        source_port: u16,
        destination_port: u16,
        rx: SAsyncPacketRx,
        tx: MAsyncTx<mpsc::Array<WriterMsg>>,
    ) -> Result<Self, RusbmuxError> {
        let conn_state =
            ConnState::perform_handshake(source_port, destination_port, &rx, &tx).await?;

        let sendable_bytes = conn_state.calc_sendable_bytes();

        Ok(Self {
            device_core: device.core(),
            source_port,
            destination_port,
            sendable_bytes,
            conn_state,
            rx,
            tx,
            dropped: false,
        })
    }

    /// you must include the length prefix at the start
    pub async fn send_bytes(&mut self, value: Bytes) -> Result<(), RusbmuxError> {
        let packet = self.build_bytes(value);

        self.send_packet(packet).await
    }

    pub async fn send_plist(&mut self, value: plist::Value) -> Result<(), RusbmuxError> {
        let packet = UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.conn_state.sent_bytes,
                self.conn_state.received_bytes,
                TcpFlags::ACK,
            )
            .payload_plist(value)
            .build();

        self.send_packet(packet).await
    }

    async fn send_packet(&mut self, packet: UsbDevicePacket) -> Result<(), RusbmuxError> {
        let payload_len = packet.payload.len() as u32;

        self.tx.send(WriterMsg::Packet(packet)).await?;

        self.add_sent_bytes(payload_len);

        trace!(
            source_port = self.source_port,
            destination_port = self.destination_port,
            len = payload_len,
            "Sent payload"
        );

        self.update_sendable_bytes();

        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), RusbmuxError> {
        self.send_rst().await?;

        let (tx, rx) = crossfire::oneshot::oneshot();
        self.tx.send(WriterMsg::Flush(tx)).await?;

        // NOTE: this future would resolve on either the loop send back, or BOTH of the device's tx
        // and rx gets dropped, which would drop the whole channel with
        // its items, thus dropping the tx of this oneshot,
        // thus waking up the rx.
        //
        // The device's rx would definitaly drop on device drop, but the tx is shared
        // through the UsbDeviceShared and implements clone, so if a UsbDeviceShared is out there
        // while the device got dropped (triggers a panic in debug build), then this would block
        // until all clones are dropped
        let _ = rx.await;

        unsafe { self.set_dropped() };

        Ok(())
    }

    pub fn close_blocking(&mut self) -> Result<(), RusbmuxError> {
        self.send_rst_blocking()?;

        let (tx, rx) = crossfire::oneshot::oneshot();
        self.tx.try_send(WriterMsg::Flush(tx))?;
        // this would block the loop if they're in the same thread
        let _ = rx.recv();

        unsafe { self.set_dropped() };

        Ok(())
    }

    pub(crate) fn send_rst_blocking(&mut self) -> Result<(), RusbmuxError> {
        debug!(
            source_port = self.source_port,
            destination_port = self.destination_port,
            "Closing connection"
        );

        let rst_packet = self.build_flag(TcpFlags::RST);

        self.tx.try_send(WriterMsg::Packet(rst_packet))?;

        trace!(
            source_port = self.source_port,
            destination_port = self.destination_port,
            "Sent RST"
        );
        Ok(())
    }

    pub async fn send_rst(&mut self) -> Result<(), RusbmuxError> {
        debug!(
            source_port = self.source_port,
            destination_port = self.destination_port,
            "Closing connection"
        );

        let rst_packet = self.build_flag(TcpFlags::RST);

        self.tx.send(WriterMsg::Packet(rst_packet)).await?;

        trace!(
            source_port = self.source_port,
            destination_port = self.destination_port,
            "Sent RST"
        );
        Ok(())
    }

    async fn ack(&mut self) -> Result<(), RusbmuxError> {
        let tcp_ack = self.build_flag(TcpFlags::ACK);

        self.tx.send(WriterMsg::Packet(tcp_ack)).await?;

        self.update_sendable_bytes();

        trace!(
            source_port = self.source_port,
            destination_port = self.destination_port,
            "Sent ACK"
        );
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<UsbDevicePacket, RusbmuxError> {
        let response = self.rx.recv().await?;

        self.update_states(&response);

        // WARN: this is not cancellation-safe, what if the other branch in a `select!`
        // completed while we are sending the ack?, then the response is dropped
        self.ack().await?;

        Ok(response)
    }

    #[inline]
    pub(crate) fn build_bytes(&self, value: Bytes) -> UsbDevicePacket {
        UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.conn_state.sent_bytes,
                self.conn_state.received_bytes,
                TcpFlags::ACK,
            )
            .payload_bytes(value)
            .build()
    }

    #[inline]
    pub(crate) fn build_flag(&self, tcp_flag: TcpFlags) -> UsbDevicePacket {
        // NOTE: can this be pre-built for performance? does it matter?
        UsbDevicePacket::builder()
            .header_tcp(AUTO_SEQ, AUTO_SEQ)
            .tcp_header(
                self.source_port,
                self.destination_port,
                self.conn_state.sent_bytes,
                self.conn_state.received_bytes,
                tcp_flag,
            )
            .build()
    }

    pub(crate) fn update_states(&mut self, packet: &UsbDevicePacket) {
        let recv_bytes = packet.payload.len() as u32;

        if let Some(ref tcp_hdr) = packet.tcp_hdr {
            self.conn_state.received_bytes = tcp_hdr.sequence_number.wrapping_add(recv_bytes);

            self.conn_state.device_received_bytes = tcp_hdr.acknowledgment_number;
            self.conn_state.device_window_size = tcp_hdr.window_size;
        }
    }

    pub(crate) fn update_sendable_bytes(&mut self) {
        let sendable = self.conn_state.calc_sendable_bytes();

        self.sendable_bytes = sendable;

        trace!("new sendable amount of bytes: {sendable}",);
    }

    pub async fn wait_shutdown(&self) {
        self.device_core.canceler.cancelled().await;
    }
}

// atomic setters and getters
impl UsbDeviceConn {
    #[inline]
    pub fn get_sendable_bytes(&self) -> usize {
        self.sendable_bytes
    }

    #[inline]
    pub(crate) fn add_sent_bytes(&mut self, value: u32) {
        self.conn_state.sent_bytes = self.conn_state.sent_bytes.wrapping_add(value);

        trace!(
            source_port = self.source_port,
            destination_port = self.destination_port,
            bytes = value,
            total = self.conn_state.sent_bytes,
            "Updated sent bytes"
        );
    }

    #[inline]
    pub fn dropped(&self) -> bool {
        self.dropped
    }

    /// # Safety
    ///
    /// setting the device as dropped would prevent it from a clean shutdown, make sure you already
    /// sent an RST packet
    #[inline]
    pub unsafe fn set_dropped(&mut self) {
        self.dropped = true;
    }

    #[inline]
    pub fn device_id(&self) -> u64 {
        self.device_core.id
    }

    #[inline]
    pub fn destination_port(&self) -> u16 {
        self.destination_port
    }

    #[inline]
    pub fn source_port(&self) -> u16 {
        self.source_port
    }

    #[inline]
    pub fn get_tx(&self) -> MAsyncTx<crossfire::mpsc::Array<WriterMsg>> {
        self.tx.clone()
    }

    #[inline]
    pub fn device_canceler(&self) -> CancellationToken {
        self.device_core.canceler.clone()
    }
}

impl Drop for UsbDeviceConn {
    fn drop(&mut self) {
        if self.dropped() {
            return;
        }

        if self.device_core.canceler.is_cancelled() {
            debug!(
                source_port = self.source_port,
                "Connection dropped after the device was dropped"
            );
            return;
        } else {
            warn!(
                source_port = self.source_port,
                "Connection dropped without an explicit close"
            );
        }

        // can't flush in drop, because if both the connection and the device loop are in the
        // same thread, it's a deadlock
        if let Err(err) = self.send_rst_blocking() {
            match err {
                RusbmuxError::Channel(crate::error::ChannelError::UsbSend(_)) => {
                    debug!(
                        "Failed to send RST because the device may already be gone; \
                         this is expected if `close_all` or `shutdown` was called"
                    );
                }
                err => {
                    debug!(
                        %err,
                        "Failed to send RST while dropping the connection"
                    );
                }
            }
        }
    }
}
