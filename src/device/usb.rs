use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
};

use bytes::Bytes;
use crossfire::{AsyncRx, MAsyncTx, mpsc};
use etherparse::TcpHeader;
use pack1::U16BE;
use tokio::{io::AsyncWriteExt, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::{
    conn::UsbDeviceConn,
    device::{WriterMsg, core::DeviceCore, packet_router::PacketRouter},
    error::{ParseError, RusbmuxError},
    parser::device_mux::{
        AUTO_SEQ, TcpFlags, UsbDevicePacket, UsbDevicePacketHeader, UsbDevicePacketHeaderV2,
        UsbDevicePacketPayload,
    },
    usb_backend::{AnyDeviceHandle, AnyDeviceInfo, AnyEndpointReader, AnyEndpointWriter},
};

#[derive(Debug, Clone, Copy)]
pub struct IODisconnectedDevice {
    pub id: u64,
    pub opaque_id: u64,
}

#[derive(Debug, Clone)]
pub struct UsbDeviceShared {
    pub(crate) core: DeviceCore,
    pub(crate) router: Arc<PacketRouter>,
    writer_tx: MAsyncTx<mpsc::Array<WriterMsg>>,
    udid: Option<String>,
}

// TODO: doc about the drop
#[derive(Debug)]
pub struct UsbDevice {
    shared: UsbDeviceShared,
    _handler: AnyDeviceHandle,

    speed: Option<u64>,
    location_id: u32,
    product_id: u16,

    reader_loop_handler: JoinHandle<()>,
    writer_loop_handler: JoinHandle<()>,

    // TODO: use the canceler
    dropped: bool,
}

impl UsbDevice {
    pub async fn new(info: AnyDeviceInfo, id: u64) -> Result<Self, RusbmuxError> {
        Self::new_with_disconnect_tx(info, id, None).await
    }

    pub async fn new_with_disconnect_tx(
        info: AnyDeviceInfo,
        id: u64,
        disconnected_tx: Option<MAsyncTx<mpsc::Array<IODisconnectedDevice>>>,
    ) -> Result<Self, RusbmuxError> {
        debug!(device_id = id, "Creating new device");
        let udid = info.udid().map(Cow::into_owned);
        let device_handle = info.open().await?;

        let (mut end_in, mut end_out) = device_handle.endpoint().await?;

        let version_packet = UsbDevicePacket::builder()
            .header_version()
            .payload_version(2, 0)
            .build();

        end_out.write_all(&version_packet.encode()).await?;
        end_out.flush().await?;

        debug!(device_id = id, "Sent version packet");

        // devices sometimes send packets from other unclosed connections
        let version = tokio::time::timeout(tokio::time::Duration::from_secs(3), async {
            loop {
                let version_response = UsbDevicePacket::from_reader(&mut end_in).await?;

                match version_response.payload {
                    UsbDevicePacketPayload::Version(v) => break Result::<_, RusbmuxError>::Ok(v),
                    _ => {
                        warn!("Received a non version packet, dropping");
                        continue;
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            RusbmuxError::UnexpectedPacket(
                "Expected to receive a version packet within 3s".to_string(),
            )
        })??;

        debug!(device_id = id, ?version, "Received version response");

        let setup_packet = UsbDevicePacket::builder()
            .header_setup()
            .payload_bytes(Bytes::from_static(&[0x07]))
            .build();

        end_out.write_all(&setup_packet.encode()).await?;
        end_out.flush().await?;

        debug!(device_id = id, "Sent setup packet");

        let (tx, rx) = mpsc::bounded_async(256);

        let router = Arc::new(PacketRouter::new());
        let router2 = Arc::clone(&router);

        let core = DeviceCore::new(id);

        let recv_seq = Arc::new(AtomicU16::new(0));
        let recv_seq2 = Arc::clone(&recv_seq);

        let opaque_id = info.opaque_id();

        info!(device_id = id, "Spawning reader & writer loops");

        let reader_loop_handler = tokio::spawn(Self::start_reader_loop(
            router2,
            recv_seq2,
            disconnected_tx,
            end_in,
            id,
            opaque_id,
        ));

        let writer_loop_handler = tokio::spawn(Self::start_writer_loop(recv_seq, rx, end_out, id));

        debug!(device_id = id, "Device created");

        Ok(Self {
            shared: UsbDeviceShared {
                core,
                router,
                writer_tx: tx,
                udid,
            },
            _handler: device_handle,
            speed: info.speed(),
            location_id: info.location_id(),
            product_id: info.product_id(),
            reader_loop_handler,
            writer_loop_handler,
            dropped: false,
        })
    }

    async fn start_reader_loop(
        router: Arc<PacketRouter>,
        recv_seq: Arc<AtomicU16>,
        disconnected_tx: Option<MAsyncTx<mpsc::Array<IODisconnectedDevice>>>,
        mut end_in: AnyEndpointReader,
        device_id: u64,
        opaque_id: u64,
    ) {
        info!(target: "device_reader", device_id, "Reader loop started");
        loop {
            trace!(target: "device_reader", device_id, "Waiting for a packet");
            let packet = match UsbDevicePacket::from_reader(&mut end_in).await {
                Ok(p) => p,

                // if it's an io, then the device probably got disconnected
                Err(ParseError::IO(err)) => {
                    warn!(target: "device_reader", device_id, %err, "Failed to read packet, closing device");

                    // some io disconnections don't report back a udev disconnected event
                    if let Some(tx) = disconnected_tx {
                        let _ = tx
                            .send(IODisconnectedDevice {
                                id: device_id,
                                opaque_id,
                            })
                            .await;
                    }

                    // clearing the router drops the tx of the connection
                    // thus waking up the rx with an error
                    router.clear();
                    break;
                }

                Err(err) => {
                    error!(target: "device_reader", device_id, %err, "Failed to read packet");
                    // WARN: is it okay to continue?
                    continue;
                }
            };

            recv_seq.fetch_add(1, Ordering::Relaxed);

            if let Some(t) = packet.tcp_hdr.as_ref()
                && t.rst
            {
                error!(
                    target: "device_reader",
                    device_id,
                    port = t.source_port,
                    payload = ?packet.payload.as_bytes(),
                    "Received TCP RST"
                );

                let port = t.destination_port;
                router.unregister(port);

                continue;
            } else if let UsbDevicePacketPayload::Error {
                error_code,
                message,
            } = &packet.payload
            {
                error!(
                    target: "device_reader",
                    device_id,
                    tcp_hdr = ?packet.tcp_hdr,
                    error_code = ?error_code,
                    message = ?message,
                    "Received an error packet"
                );
                router.route(packet).await;
                continue;
            }

            debug!(
                target: "device_reader",
                device_id,
                payload = ?packet.payload.as_bytes(),
                len = packet.header.get_length(),
                "Received a packet from the device"
            );

            router.route(packet).await;
        }
    }

    async fn start_writer_loop(
        recv_seq: Arc<AtomicU16>,
        rx: AsyncRx<mpsc::Array<WriterMsg>>,
        mut end_out: AnyEndpointWriter,
        device_id: u64,
    ) {
        let mut hbuf = [0; UsbDevicePacketHeaderV2::SIZE + TcpHeader::MIN_LEN];

        info!(target: "device_writer", device_id, "Writer loop started");

        // starts at 1 because we sent the syn
        let mut send_seq = 1;

        loop {
            trace!(target: "device_writer", device_id, "Waiting for a packet");
            let mut packet = match rx.recv().await {
                Ok(WriterMsg::Packet(p)) => p,
                Ok(WriterMsg::Flush(tx)) => {
                    let _ = end_out.flush().await;
                    tx.send(());
                    continue;
                }
                Err(_) => {
                    error!(target: "device_writer", device_id, "Writer channel closed");
                    break;
                }
            };

            debug!(
                target: "device_writer",
                device_id,
                payload = ?packet.payload.as_bytes(),
                "Received a packet from the client"
            );

            if let UsbDevicePacketHeader::V2(v2) = &mut packet.header {
                let recv_seq_ = recv_seq.load(Ordering::Relaxed);

                v2.send_seq = U16BE::new(send_seq);
                v2.recv_seq = U16BE::new(recv_seq_);

                trace!(target: "device_writer", device_id, send_seq, recv_seq=recv_seq_, "Updating seq numbers");

                send_seq = send_seq.wrapping_add(1);
            }

            trace!(target: "device_writer", device_id, "Encoding headers");
            match packet.header {
                UsbDevicePacketHeader::V1(h) => {
                    if let Err(err) = end_out.write_all(h.encode()).await {
                        if !crate::utils::is_disconnect_io(&err) {
                            error!(target: "device_writer", device_id, %err, "Failed to write packet header v1");
                        }
                        continue;
                    }
                }
                UsbDevicePacketHeader::V2(h) => {
                    hbuf[..UsbDevicePacketHeaderV2::SIZE].copy_from_slice(h.encode());

                    if let Some(tcp_hdr) = packet.tcp_hdr.as_ref() {
                        hbuf[UsbDevicePacketHeaderV2::SIZE..].copy_from_slice(&tcp_hdr.to_bytes());

                        if let Err(err) = end_out.write_all(&hbuf).await {
                            if !crate::utils::is_disconnect_io(&err) {
                                error!(target: "device_writer", device_id, %err, "Failed to write packet header v2");
                            }
                            continue;
                        }
                    } else if let Err(err) = end_out
                        .write_all(&hbuf[..UsbDevicePacketHeaderV2::SIZE])
                        .await
                    {
                        if !crate::utils::is_disconnect_io(&err) {
                            error!(target: "device_writer", device_id, %err, "Failed to write packet header v2");
                        }
                        continue;
                    }
                }
            }

            let payload = packet.payload.encode();

            trace!(target: "device_writer", device_id, len = payload.len(), "Writing payload");

            if let Err(err) = end_out.write_all(&payload).await
                && !crate::utils::is_disconnect_io(&err)
            {
                error!(target: "device_writer", device_id, %err, "Failed to write packet payload");
            }

            end_out.submit_end();
        }

        let _ = end_out.flush().await;
    }

    pub async fn connect(&self, destination_port: u16) -> Result<UsbDeviceConn, RusbmuxError> {
        let (source_port, rx) = self.shared.router.register(destination_port)?;

        debug!(
            device_id = self.shared.core.id,
            source_port, destination_port, "Creating new connection"
        );

        let conn = match UsbDeviceConn::new(
            self,
            source_port,
            destination_port,
            rx,
            self.shared.writer_tx.clone(),
        )
        .await
        {
            Ok(c) => c,
            Err(err) => {
                self.shared.router.unregister(source_port);
                return Err(err);
            }
        };

        Ok(conn)
    }

    pub async fn close_all(&self) -> Result<(), RusbmuxError> {
        debug!(device_id = self.shared.core.id, "Closing all connections");

        for entry in self.shared.router.take_all_alive_entries() {
            let rst_packet = UsbDevicePacket::builder()
                .header_tcp(AUTO_SEQ, AUTO_SEQ)
                .tcp_header(
                    entry.source_port,
                    entry.destination_port,
                    // apparently you can reset the connection with just the src and dst port
                    //
                    // I've already tested this, and yeah I can't send nor recv on the connection aftward,
                    // and the device returns "\x01handleMuxTCPInput no matching session" once doing so
                    //
                    // NOTE: this was only tested on my device, maybe that's not the case on
                    // different newer/older devices?
                    0,
                    0,
                    TcpFlags::RST,
                )
                .build();

            // only fails if the receiver is gone, which means the loop is gone
            self.shared
                .writer_tx
                .send(WriterMsg::Packet(rst_packet))
                .await?;
        }

        let (tx, rx) = crossfire::oneshot::oneshot();
        self.shared.writer_tx.send(WriterMsg::Flush(tx)).await?;
        let _ = rx.await;

        Ok(())
    }

    pub fn close_all_blocking(&self) -> Result<(), RusbmuxError> {
        debug!(device_id = self.shared.core.id, "Closing all connections");

        let mut first_err = None;
        for entry in self.shared.router.take_all_alive_entries() {
            let rst_packet = UsbDevicePacket::builder()
                .header_tcp(AUTO_SEQ, AUTO_SEQ)
                .tcp_header(
                    entry.source_port,
                    entry.destination_port,
                    0,
                    0,
                    TcpFlags::RST,
                )
                .build();

            if let Err(err) = self
                .shared
                .writer_tx
                .try_send(WriterMsg::Packet(rst_packet))
            {
                first_err.get_or_insert(err);
            }
        }

        if let Some(err) = first_err {
            return Err(err.into());
        }

        let (tx, rx) = crossfire::oneshot::oneshot();
        self.shared.writer_tx.try_send(WriterMsg::Flush(tx))?;
        let _ = rx.recv();

        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), RusbmuxError> {
        self.shared.core.canceler.cancel();
        let result = self.close_all().await;
        self.dropped = true;
        self.drop_loops();

        result
    }

    pub fn shutdown_blocking(&mut self) -> Result<(), RusbmuxError> {
        self.shared.core.canceler.cancel();
        let result = self.close_all_blocking();
        self.dropped = true;
        self.drop_loops();

        result
    }

    fn drop_loops(&self) {
        debug!(device_id = self.shared.core.id, "Aborting reader loop");
        self.reader_loop_handler.abort();

        debug!(device_id = self.shared.core.id, "Aborting writer loop");
        self.writer_loop_handler.abort();
    }

    fn abort(&mut self) {
        self.shared.core.canceler.cancel();

        for entry in self.shared.router.take_all_alive_entries() {
            let rst = UsbDevicePacket::builder()
                .header_tcp(AUTO_SEQ, AUTO_SEQ)
                .tcp_header(
                    entry.source_port,
                    entry.destination_port,
                    0,
                    0,
                    TcpFlags::RST,
                )
                .build();

            if let Err(err) = self.shared.writer_tx.try_send(WriterMsg::Packet(rst)) {
                debug!(%err, "Failed to send the RST while aborting the device");
            }
        }

        // NOTE: does it matter to send an rst if we immediately drop the loops?
        self.drop_loops();
    }

    #[inline]
    pub fn core(&self) -> DeviceCore {
        self.shared.core.clone()
    }

    #[inline]
    pub fn id(&self) -> u64 {
        self.shared.core.id
    }

    #[inline]
    pub fn udid(&self) -> Option<&str> {
        self.shared.udid()
    }

    #[inline]
    pub fn shared(&self) -> UsbDeviceShared {
        self.shared.clone()
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        let router_owners = Arc::strong_count(&self.shared.router);

        debug_assert!(
            // one in the reader loop, and the other is ours, if there's more, then it's probably
            // an RusbmuxProvider somewhere
            router_owners <= 2,
            "USB device dropped while packet router is still shared \
            (strong references: {router_owners}, expected at most 2; \
            an RusbmuxProvider may still be alive)"
        );

        if !self.dropped {
            warn!("Device dropped without an explicit shutdown");
            self.abort();
            self.dropped = true;
        }
    }
}

impl UsbDevice {
    pub fn create_device_attached(&self) -> plist::Value {
        let location_id = self.location_id;

        let speed = self.speed.unwrap_or(0);
        let udid = &self.shared.udid;

        debug!(
            device_id = self.shared.core.id,
            ?udid,
            speed,
            location_id,
            product_id = self.product_id,
            "Adding device to plist"
        );

        plist_macro::plist!({
            "DeviceID": self.shared.core.id,
            "MessageType": "Attached",
            "Properties": {
                "ConnectionSpeed": speed,
                "ConnectionType": "USB",
                "DeviceID": self.shared.core.id,
                "LocationID": location_id,
                "ProductID": self.product_id,
                "SerialNumber":? udid.as_ref(),
            }
        })
    }
}

impl UsbDeviceShared {
    pub fn udid(&self) -> Option<&str> {
        self.udid.as_deref()
    }

    pub fn id(&self) -> u64 {
        self.core.id
    }

    pub fn get_writer(&self) -> MAsyncTx<crossfire::mpsc::Array<WriterMsg>> {
        self.writer_tx.clone()
    }

    pub fn canceler(&self) -> CancellationToken {
        self.core.canceler.clone()
    }
}
