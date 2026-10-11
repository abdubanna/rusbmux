use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU16, Ordering},
};

use bytes::Bytes;
use crossfire::{AsyncRx, MAsyncTx, mpsc};
use etherparse::TcpHeader;
use pack1::U16BE;
use tokio::{io::AsyncWriteExt, task::JoinHandle};
use tracing::{debug, error, info, trace, warn};

use crate::{
    conn::UsbDeviceConn,
    device::{core::DeviceCore, packet_router::PacketRouter},
    error::{ParseError, RusbmuxError},
    parser::device_mux::{
        UsbDevicePacket, UsbDevicePacketHeader, UsbDevicePacketHeaderV2, UsbDevicePacketPayload,
        UsbDevicePacketVersion,
    },
    usb_backend::{
        AnyDeviceHandle, AnyDeviceInfo, AnyEndpointReader, AnyEndpointWriter, UsbAsyncWriteEndpoint,
    },
};

#[derive(Debug, Clone, Copy)]
pub struct IODisconnectedDevice {
    pub id: u64,
    pub opaque_id: u64,
}

#[derive(Debug, Clone)]
pub struct UsbDeviceShared {
    pub core: DeviceCore,
    pub router: Arc<PacketRouter>,
    pub writer_tx: MAsyncTx<mpsc::Array<UsbDevicePacket>>,
    pub udid: String,
}

#[derive(Debug)]
pub struct UsbDevice {
    pub shared: UsbDeviceShared,
    pub handler: AnyDeviceHandle,

    pub speed: Option<u64>,
    pub location_id: u32,
    pub product_id: u16,

    pub version: UsbDevicePacketVersion,

    reader_loop_handler: JoinHandle<()>,
    writer_loop_handler: JoinHandle<()>,

    dropped: AtomicBool,
}

impl UsbDevice {
    /// # Safety
    ///
    /// make sure you already sent the `UsbDevicePacketProtocol::Setup` packet
    pub async unsafe fn new_from(
        info: AnyDeviceInfo,
        id: u64,
        version: UsbDevicePacketVersion,
        disconnected_tx: Option<MAsyncTx<mpsc::Array<IODisconnectedDevice>>>,
    ) -> Result<Self, RusbmuxError> {
        debug!(device_id = id, "Creating device from existing state");
        let udid = info
            .udid()
            .ok_or(RusbmuxError::InvalidData("USB device has no serial number"))?
            .into_owned();
        let device_handle = info.open().await?;

        let (end_in, end_out) = device_handle.endpoint().await?;

        let (tx, rx) = mpsc::bounded_async(256);

        let router = Arc::new(PacketRouter::new());
        let router2 = Arc::clone(&router);

        let core = DeviceCore::new(id);

        let recv_seq = Arc::new(AtomicU16::new(0));
        let recv_seq2 = Arc::clone(&recv_seq);

        let canceler = core.canceler.clone();
        let canceler2 = core.canceler.clone();

        let opaque_id = info.opaque_id();

        info!(device_id = id, "Spawning reader & writer loops");

        let reader_loop_handler = tokio::spawn(async move {
            tokio::select! {
                _ = Self::start_reader_loop(router2, recv_seq2, disconnected_tx,  end_in, id, opaque_id) => {}
                _ = canceler.cancelled() => {}
            }
        });

        let writer_loop_handler = tokio::spawn(async move {
            tokio::select! {
                _ = Self::start_writer_loop(recv_seq, rx, end_out, id) => {}
                _ = canceler2.cancelled() => {}
            }
        });

        debug!(device_id = id, "Device created");

        Ok(Self {
            shared: UsbDeviceShared {
                core,
                router,
                writer_tx: tx,
                udid,
            },
            handler: device_handle,
            speed: info.speed(),
            location_id: info.location_id(),
            product_id: info.product_id(),
            version,
            reader_loop_handler,
            writer_loop_handler,
            dropped: AtomicBool::new(false),
        })
    }

    pub async fn new(info: AnyDeviceInfo, id: u64) -> Result<Self, RusbmuxError> {
        Self::new_with_disconnect_tx(info, id, None).await
    }

    pub async fn new_with_disconnect_tx(
        info: AnyDeviceInfo,
        id: u64,
        disconnected_tx: Option<MAsyncTx<mpsc::Array<IODisconnectedDevice>>>,
    ) -> Result<Self, RusbmuxError> {
        debug!(device_id = id, "Creating new device");
        let udid = info
            .udid()
            .ok_or(RusbmuxError::InvalidData("USB device has no serial number"))?
            .into_owned();
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
        //
        // TODO: add timeout
        let version = loop {
            let version_response = UsbDevicePacket::from_reader(&mut end_in).await?;

            match version_response.payload {
                UsbDevicePacketPayload::Version(v) => break v,
                _ => {
                    warn!("Received a non version packet, dropping");
                    continue;
                }
            }
        };

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

        let canceler = core.canceler.clone();
        let canceler2 = core.canceler.clone();

        let opaque_id = info.opaque_id();

        info!(device_id = id, "Spawning reader & writer loops");

        let reader_loop_handler = tokio::spawn(async move {
            tokio::select! {
                _ = Self::start_reader_loop(router2, recv_seq2, disconnected_tx,  end_in, id, opaque_id) => {}
                _ = canceler.cancelled() => {}
            }
        });

        let writer_loop_handler = tokio::spawn(async move {
            tokio::select! {
                _ = Self::start_writer_loop(recv_seq, rx, end_out, id) => {}
                _ = canceler2.cancelled() => {}
            }
        });

        debug!(device_id = id, "Device created");

        Ok(Self {
            shared: UsbDeviceShared {
                core,
                router,
                writer_tx: tx,
                udid,
            },
            handler: device_handle,
            speed: info.speed(),
            location_id: info.location_id(),
            product_id: info.product_id(),
            version,
            reader_loop_handler,
            writer_loop_handler,
            dropped: AtomicBool::new(false),
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
                    //
                    // TODO: test that the connections gets removed
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
        rx: AsyncRx<mpsc::Array<UsbDevicePacket>>,
        mut end_out: AnyEndpointWriter,
        device_id: u64,
    ) {
        let mut hbuf = [0; UsbDevicePacketHeaderV2::SIZE + TcpHeader::MIN_LEN];

        info!(target: "device_writer", device_id, "Writer loop started");

        // starts at 1 because we sent the syn
        //
        // TODO: wrapping?
        let mut send_seq = 1;

        loop {
            trace!(target: "device_writer", device_id, "Waiting for a packet");
            let Ok(mut packet) = rx.recv().await else {
                error!(target: "device_writer", device_id, "Writer channel closed");
                break;
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

                send_seq += 1;
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
    }

    pub async fn connect(&self, destination_port: u16) -> Result<Arc<UsbDeviceConn>, RusbmuxError> {
        let (source_port, rx) = self.shared.router.register()?;

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

        self.shared
            .router
            .attach_conn(source_port, Arc::downgrade(&Arc::clone(&conn)));

        Ok(conn)
    }

    #[inline]
    pub fn shared(&self) -> UsbDeviceShared {
        self.shared.clone()
    }

    #[inline]
    pub fn udid(&self) -> &str {
        &self.shared.udid
    }

    pub async fn cleanup_conn(&self, conn: &UsbDeviceConn) -> Result<(), RusbmuxError> {
        let source_port = conn.source_port;

        if let Some(pe) = self.shared.router.unregister(source_port)
            && let Some(conn) = pe.conn.and_then(|conn| conn.upgrade())
            && !conn.dropped()
        {
            conn.close().await?;
        }

        Ok(())
    }

    pub async fn close_all(&self) -> Result<(), RusbmuxError> {
        debug!(device_id = self.shared.core.id, "Closing all connections");

        let mut first_err = None;
        for conn in self.shared.router.take_all_conns() {
            if let Some(conn) = conn
                && let Err(err) = conn.send_rst().await
            {
                first_err.get_or_insert(err);
            }
        }

        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    pub fn close_all_blocking(&self) -> Result<(), RusbmuxError> {
        debug!(device_id = self.shared.core.id, "Closing all connections");

        let mut first_err = None;
        for conn in self.shared.router.take_all_conns() {
            if let Some(conn) = conn
                && let Err(err) = conn.send_rst_blocking()
            {
                first_err.get_or_insert(err);
            }
        }

        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    #[inline]
    fn dropped(&self) -> bool {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    fn set_dropped(&self) {
        self.dropped
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn drop_loops(&self) {
        debug!(device_id = self.shared.core.id, "Aborting reader loop");
        self.reader_loop_handler.abort();

        debug!(device_id = self.shared.core.id, "Aborting writer loop");
        self.writer_loop_handler.abort();
    }

    pub async fn shutdown(&self) -> Result<(), RusbmuxError> {
        self.shared.core.canceler.cancel();
        self.close_all().await?;
        self.set_dropped();
        // TODO: make it wait until all packets are sent
        self.drop_loops();

        Ok(())
    }

    pub fn shutdown_blocking(&self) -> Result<(), RusbmuxError> {
        self.shared.core.canceler.cancel();
        self.close_all_blocking()?;
        self.set_dropped();
        self.drop_loops();

        Ok(())
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

        if !self.dropped() {
            let _ = self.shutdown_blocking();
        }
    }
}

impl UsbDevice {
    pub fn create_device_attached(&self) -> Result<plist::Value, RusbmuxError> {
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

        Ok(plist_macro::plist!({
            "DeviceID": self.shared.core.id,
            "MessageType": "Attached",
            "Properties": {
                "ConnectionSpeed": speed,
                "ConnectionType": "USB",
                "DeviceID": self.shared.core.id,
                "LocationID": location_id,
                "ProductID": self.product_id,
                "SerialNumber": udid,
            }
        }))
    }
}
