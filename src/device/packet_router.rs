use std::{
    ops::Deref,
    sync::{
        Arc, Weak,
        atomic::{AtomicU16, Ordering},
    },
};

use crossfire::{AsyncRx, AsyncTx, spsc};
use dashmap::DashMap;
use tracing::{debug, trace, warn};

use crate::{conn::UsbDeviceConn, error::RusbmuxError, parser::device_mux::UsbDevicePacket};

#[derive(Debug)]
pub struct SAsyncPacketTx(pub AsyncTx<spsc::Array<UsbDevicePacket>>);

#[derive(Debug)]
pub struct SAsyncPacketRx(pub AsyncRx<spsc::Array<UsbDevicePacket>>);

impl Deref for SAsyncPacketTx {
    type Target = AsyncTx<spsc::Array<UsbDevicePacket>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for SAsyncPacketRx {
    type Target = AsyncRx<spsc::Array<UsbDevicePacket>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

unsafe impl Sync for SAsyncPacketTx {}
unsafe impl Sync for SAsyncPacketRx {}

type SourcePort = u16;

#[derive(Debug)]
pub struct PacketEntry {
    pub tx: SAsyncPacketTx,
    /// bookkeeping for `close_all`; weak, so it never keeps a conn alive
    pub conn: Option<Weak<UsbDeviceConn>>,
}

#[derive(Debug)]
pub struct PacketRouter {
    pub routes: DashMap<SourcePort, PacketEntry>,
    pub next_source_port: AtomicU16,
}

impl Default for PacketRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketRouter {
    const MAX_SOURCE_PORT_PROBES: u32 = 64;

    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: DashMap::new(),
            next_source_port: AtomicU16::new(1),
        }
    }

    pub fn cleanup_dead(&self) {
        self.routes.retain(|port, conn| {
            let dead = conn.tx.is_disconnected();
            if dead {
                debug!(port, "Removing dead connection");
            }

            !dead
        });
    }

    pub fn take_all_conns(&self) -> Vec<Option<Arc<UsbDeviceConn>>> {
        self.routes
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|key| self.routes.remove(&key))
            .map(|(_, entry)| entry.conn)
            .filter_map(|conn| conn.map(|c| c.upgrade()))
            .collect()
    }

    pub fn attach_conn(&self, source_port: u16, conn: Weak<UsbDeviceConn>) {
        if let Some(mut pe) = self.routes.get_mut(&source_port) {
            pe.conn = Some(conn)
        }
    }

    pub fn register(&self) -> Result<(SourcePort, SAsyncPacketRx), RusbmuxError> {
        self.cleanup_dead();

        for _ in 0..Self::MAX_SOURCE_PORT_PROBES {
            let sp = self.next_source_port.fetch_add(1, Ordering::Relaxed);
            let sp = if sp == 0 { 1 } else { sp };
            if let dashmap::Entry::Vacant(v) = self.routes.entry(sp) {
                let (tx, rx) = spsc::bounded_async(256);
                v.insert(PacketEntry {
                    tx: SAsyncPacketTx(tx),
                    conn: None,
                });

                debug!(port = sp, "Connection registered");
                return Ok((sp, SAsyncPacketRx(rx)));
            }
        }

        warn!(
            "Source ports wrapped around without finding a free port within {} probes",
            Self::MAX_SOURCE_PORT_PROBES
        );

        Err(RusbmuxError::RanOutOfSourcePort)
    }

    #[inline]
    pub fn clear(&self) {
        self.routes.clear();
    }

    #[inline]
    pub fn unregister(&self, port: u16) -> Option<PacketEntry> {
        let pe = self.routes.remove(&port);
        debug!(port, "Connection unregistered");

        pe.map(|(_, pe)| pe)
    }

    pub async fn route(&self, packet: UsbDevicePacket) {
        let port = packet.tcp_hdr.as_ref().map_or(0, |h| h.destination_port);

        trace!(port, "Routing packet");

        // NOTE: the `conn` is held while the `.send` is finished
        if let Some(conn) = self.routes.get(&port) {
            if conn.tx.send(packet).await.is_err() {
                warn!(port, "Connection dropped (receiver gone), unregistering");
                drop(conn);
                self.unregister(port);
            }
        } else {
            trace!(port, "No connection found, dropping packet");
        }
    }
}
