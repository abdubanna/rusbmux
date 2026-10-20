use std::{
    ops::Deref,
    sync::atomic::{AtomicU16, Ordering},
};

use crossfire::{AsyncRx, AsyncTx, spsc};
use dashmap::DashMap;
use tracing::{debug, trace, warn};

use crate::{error::RusbmuxError, parser::device_mux::UsbDevicePacket};

#[derive(Debug)]
pub struct SAsyncPacketTx(AsyncTx<spsc::Array<UsbDevicePacket>>);

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
    tx: SAsyncPacketTx,

    // used to close the connection in `close_all`
    pub source_port: u16,
    pub destination_port: u16,
}

#[derive(Debug)]
pub struct PacketRouter {
    routes: DashMap<SourcePort, PacketEntry>,
    next_source_port: AtomicU16,
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

    fn cleanup_dead(&self) {
        self.routes.retain(|port, conn| {
            let dead = conn.tx.is_disconnected();
            if dead {
                debug!(port, "Removing dead connection");
            }

            !dead
        });
    }

    pub fn take_all_alive_entries(&self) -> impl Iterator<Item = PacketEntry> {
        self.routes
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|key| self.routes.remove(&key))
            .map(|(_, entry)| entry)
            .filter(|entry| !entry.tx.is_disconnected())
    }

    pub fn register(
        &self,
        destination_port: u16,
    ) -> Result<(SourcePort, SAsyncPacketRx), RusbmuxError> {
        self.cleanup_dead();

        for _ in 0..Self::MAX_SOURCE_PORT_PROBES {
            let sp = self.next_source_port.fetch_add(1, Ordering::Relaxed);
            let sp = if sp == 0 { 1 } else { sp };

            if let dashmap::Entry::Vacant(v) = self.routes.entry(sp) {
                let (tx, rx) = spsc::bounded_async(256);
                v.insert(PacketEntry {
                    tx: SAsyncPacketTx(tx),
                    source_port: sp,
                    destination_port,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clear_removes_every_route_and_wakes_receivers() {
        let router = PacketRouter::new();

        let (_port, rx) = router.register(1).expect("register");
        assert_eq!(router.routes.len(), 1);

        // simulate a connection waiting for a response
        let recv_handle = tokio::spawn(async move { rx.recv().await });

        // this is what `UsbDevice::start_reader_loop` does on an IO failure
        router.clear();

        assert!(router.routes.is_empty());

        // every sender is gone, so the readers must be woken with an error instead of
        // hanging forever
        assert!(recv_handle.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn unregister_only_touches_one_route() {
        let router = PacketRouter::new();
        let (port, rx) = router.register(1).unwrap();
        let (_port2, _rx2) = router.register(1).unwrap();

        assert!(router.unregister(port).is_some());
        assert_eq!(router.routes.len(), 1);

        assert!(router.unregister(port).is_none());

        // the entry (and its tx) is gone, so the reader is released
        assert!(rx.0.recv().await.is_err());
    }

    #[tokio::test]
    async fn cleanup_dead_drops_entries_whose_receiver_is_gone() {
        let router = PacketRouter::new();
        let (port, rx) = router.register(1).unwrap();
        let (_port2, _rx2) = router.register(1).unwrap();
        assert_eq!(router.routes.len(), 2);

        drop(rx); // simulate the client side dropping the connection

        router.cleanup_dead();
        assert!(!router.routes.contains_key(&port));
    }
}
