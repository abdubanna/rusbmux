use std::{collections::HashMap, net::IpAddr, pin::Pin};

use futures::{Stream, StreamExt, stream::FuturesUnordered};
use idevice::{
    mdns::{decode_auth_tag, derive_auth_tag},
    pairing_file::PairingFile,
};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use tracing::{debug, info, warn};

use crate::{device::Device, error::RusbmuxError, usb_backend::next_device_id};

#[derive(Debug)]
pub enum NetworkEvent {
    Connected(Device),
    Disconnected(u64),
}

const SERVICE_TYPE: &str = "_apple-mobdev2._tcp.local.";

/// Watches for network devices
///
/// `pairing_files` is the source of pairing records used to identify
/// network devices.
///
/// Use [`PairingFiles::static_files`]
/// for a fixed list, or [`PairingFiles::dynamic`]
/// to reload on each discovery.
///
///
/// A static collection can be passed directly:
///
/// ```rust,no_run
/// # use idevice::pairing_file::PairingFile;
/// # use rusbmux::watcher::{watch_network, PairingFiles};
///
/// let pairing_files = vec![
///     PairingFile::from_bytes(todo!()).unwrap(),
///     PairingFile::from_bytes(todo!()).unwrap(),
/// ];
///
/// watch_network(PairingFiles::static_files(pairing_files));
/// ```
///
/// For a dynamically changing source, use a function:
///
/// ```rust,no_run
/// # use idevice::pairing_file::PairingFile;
/// # use rusbmux::watcher::{watch_network, PairingFiles};
///
/// fn get_pairing_files() -> Vec<PairingFile> {
///     todo!()
/// }
///
/// watch_network(PairingFiles::dynamic(get_pairing_files));
/// ```
pub fn watch_network(
    mut pairing_files: PairingFiles,
) -> Pin<Box<impl Stream<Item = Result<NetworkEvent, RusbmuxError>>>> {
    Box::pin(async_stream::stream! {
        let mdns = ServiceDaemon::new()?;

        let receiver = mdns.browse(SERVICE_TYPE)?;

        let mut devices_id_map = HashMap::new();

        let mut devs_canceler = FuturesUnordered::new();

        loop {
            tokio::select! {
                Ok(event) = receiver.recv_async() => {
                    match event {
                        ServiceEvent::ServiceResolved(rs) => {
                            let Some(rd) = resolve_service(rs, &mut pairing_files) else {
                                continue;
                            };

                            if devices_id_map.contains_key(&rd.mac_address) {
                                continue;
                            };

                            let id = next_device_id();

                            let device = match Device::new_network(
                                id,
                                rd.addr,
                                Some(rd.scope_id),
                                rd.service_name,
                                rd.pairing_file
                            ).await {
                                Ok(d) => {d},
                                Err(err) => {
                                    yield Err(err);
                                    continue
                                }
                            };

                            let canceler = device.as_network().unwrap().canceler();
                            let mac_address = rd.mac_address.clone();

                            devs_canceler.push(async move {
                                canceler.cancelled().await;

                                mac_address
                            });

                            devices_id_map.insert(rd.mac_address, id);

                            yield Ok(NetworkEvent::Connected(device));

                        }
                        ServiceEvent::ServiceRemoved(_, name) => {
                            let Some(mac_address) = name.split('@').next() else {
                                debug!(
                                    service_name = name,
                                    "`@` was not found in the removed service name"
                                );
                                continue;
                            };

                            if let Some(id) = devices_id_map.remove(mac_address) {
                                yield Ok(NetworkEvent::Disconnected(id))
                            };
                        }
                        _ => {}
                    }
                }

                Some(mac_address) = devs_canceler.next(), if !devs_canceler.is_empty() => {
                    if let Some(id) = devices_id_map.remove(&mac_address) {
                        yield Ok(NetworkEvent::Disconnected(id))
                    }
                }

                else => {
                    yield Err(RusbmuxError::Mdns(mdns_sd::Error::DaemonShutdown));
                    break;
                }
            }
        }
    })
}

struct ResolvedDevice {
    addr: (IpAddr, Option<IpAddr>),
    scope_id: u32,
    mac_address: String,
    service_name: String,
    pairing_file: PairingFile,
}

fn resolve_service(
    rs: Box<ResolvedService>,
    pairing_files: &mut PairingFiles,
) -> Option<ResolvedDevice> {
    debug!("Discovered network device via mDNS: {rs:#?}");
    let addresses = rs.addresses.clone();

    let ipv6 = addresses.iter().find_map(|addr| match addr {
        mdns_sd::ScopedIp::V6(addr) => Some((IpAddr::V6(*addr.addr()), addr.scope_id().index)),
        _ => None,
    });

    let ipv4 = addresses.iter().find_map(|addr| match addr {
        mdns_sd::ScopedIp::V4(addr) => Some((
            IpAddr::V4(*addr.addr()),
            addr.interface_ids().first().map_or(0, |i| i.index),
        )),
        _ => None,
    });

    // prefer ipv6 if available
    let ((primary, secondary), scope_id) = match (ipv6, ipv4) {
        (Some((v6, scope)), Some((v4, _))) => ((v6, Some(v4)), scope),
        (Some((v6, scope)), None) => ((v6, None), scope),
        (None, Some((v4, scope))) => ((v4, None), scope),
        (None, None) => return None,
    };

    // iOS 26.4+: match by Bonjour TXT record (identifier + authTag HMACs).
    let identifier = rs.get_property_val("identifier").flatten();
    let auth_tags: Vec<&[u8]> = rs
        .get_properties()
        .iter()
        .filter(|p| {
            let k = p.key();
            k == "authTag" || k.starts_with("authTag#")
        })
        .filter_map(|p| p.val())
        .collect();

    let Some(mac_address) = rs.fullname.split('@').next() else {
        warn!(
            service_name = rs.fullname,
            "`@` was not found in the service name, skipping"
        );
        return None;
    };

    // iOS 26.4+: match by Bonjour TXT record (identifier + authTag HMACs).
    let pairing_file = if let Some(ident) = identifier
        && !auth_tags.is_empty()
    {
        let Some(pf) = find_udid_from_txt(ident, &auth_tags, pairing_files) else {
            warn!("The device doesn't have a pairing file saved, skipping");
            return None;
        };

        pf
    } else {
        // iOS < 26.4 fallback: parse MAC out of the instance name (`<MAC>@<id>.…`).
        let Some(pf) = get_udid_from_mac_addr(mac_address, pairing_files) else {
            warn!(
                mac_address,
                "The device doesn't have a pairing file saved, skipping"
            );
            return None;
        };
        pf
    };

    Some(ResolvedDevice {
        addr: (primary, secondary),
        scope_id,
        mac_address: mac_address.to_string(),
        service_name: rs.fullname,
        pairing_file,
    })
}

fn find_udid_from_txt(
    identifier: &[u8],
    auth_tags: &[&[u8]],
    pairing_files: &mut PairingFiles,
) -> Option<PairingFile> {
    if auth_tags.is_empty() {
        return None;
    }

    // Decode all tags up front (they're independent of the candidate HostID).
    let decoded_tags: Vec<[u8; 8]> = auth_tags
        .iter()
        .filter_map(|t| decode_auth_tag(t))
        .collect();

    if decoded_tags.is_empty() {
        debug!("TXT record had authTag(s) but none decoded to 8 bytes");
        return None;
    }

    pairing_files.take_match(|pf| {
        // NOTE: should we skip?
        let Some(ref udid) = pf.udid else {
            return false;
        };

        let host_id = &pf.host_id;

        let expected = derive_auth_tag(host_id.as_bytes(), identifier);
        if decoded_tags.contains(&expected) {
            info!(udid, "TXT record matched UDID");
            return true;
        }

        false
    })
}

fn get_udid_from_mac_addr(mac_addr: &str, pairing_files: &mut PairingFiles) -> Option<PairingFile> {
    pairing_files.take_match(|pf| {
        // NOTE: should we skip?
        let Some(ref udid) = pf.udid else {
            return false;
        };
        if mac_addr == pf.wifi_mac_address {
            info!(udid, "WiFi mac address matched UDID");
            return true;
        }
        false
    })
}

pub enum PairingFiles {
    Static(Vec<PairingFile>),
    Dynamic(Box<dyn FnMut() -> Vec<PairingFile> + Send>),
}

impl PairingFiles {
    pub fn static_files(files: impl Into<Vec<PairingFile>>) -> Self {
        Self::Static(files.into())
    }

    pub fn dynamic(load: impl FnMut() -> Vec<PairingFile> + Send + 'static) -> Self {
        Self::Dynamic(Box::new(load))
    }

    fn take_match(&mut self, mut matches: impl FnMut(&PairingFile) -> bool) -> Option<PairingFile> {
        match self {
            Self::Static(files) => files.iter().find(|pf| matches(pf)).cloned(),
            Self::Dynamic(load) => {
                let mut files = load();
                let idx = files.iter().position(matches)?;
                Some(files.swap_remove(idx))
            }
        }
    }
}
