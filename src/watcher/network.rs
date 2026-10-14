use std::{collections::HashMap, net::IpAddr, path::Path, pin::Pin};

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

pub fn watch_network(
    // TODO: use a more friendlier way for library mode
    lockdown_path: &Path,
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
                            let Some(rd) = resolve_service(rs, lockdown_path) else {
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
                                rd.mac_address.clone(),
                                rd.service_name,
                                rd.udid,
                                lockdown_path
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
    udid: String,
}

fn resolve_service(rs: Box<ResolvedService>, lockdown_path: &Path) -> Option<ResolvedDevice> {
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
    let udid = if let Some(ident) = identifier
        && !auth_tags.is_empty()
    {
        let Some(udid) = find_udid_from_txt(ident, &auth_tags, lockdown_path) else {
            warn!("The device doesn't have a pairing file saved, skipping");
            return None;
        };

        udid
    } else {
        // iOS < 26.4 fallback: parse MAC out of the instance name (`<MAC>@<id>.…`).
        let Some(udid) = get_udid_from_mac_addr(mac_address, lockdown_path) else {
            warn!(
                mac_address,
                "The device doesn't have a pairing file saved, skipping"
            );
            return None;
        };
        udid
    };

    Some(ResolvedDevice {
        addr: (primary, secondary),
        scope_id,
        mac_address: mac_address.to_string(),
        service_name: rs.fullname,
        udid,
    })
}

fn find_udid_from_txt(
    identifier: &[u8],
    auth_tags: &[&[u8]],
    lockdown_path: &Path,
) -> Option<String> {
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

    for (udid, PairingFile { host_id, .. }) in get_saved_pairing_files(lockdown_path) {
        let expected = derive_auth_tag(host_id.as_bytes(), identifier);
        if decoded_tags.contains(&expected) {
            info!(udid, "TXT record matched UDID");
            return Some(udid);
        }
    }

    None
}

fn get_udid_from_mac_addr(mac_addr: &str, lockdown_path: &Path) -> Option<String> {
    for (
        udid,
        PairingFile {
            wifi_mac_address, ..
        },
    ) in get_saved_pairing_files(lockdown_path)
    {
        if mac_addr == wifi_mac_address {
            return Some(udid);
        }
    }

    None
}

/// gets all the valid pairing files along side it's file stem (udid)
fn get_saved_pairing_files(lockdown_path: &Path) -> Vec<(String, PairingFile)> {
    let entries = match lockdown_path.read_dir() {
        Ok(dirs) => dirs,
        Err(err) => {
            debug!(%err, "Failed to read the directories of `{lockdown_path:?}`");
            return Vec::new();
        }
    };

    entries
        .flatten()
        .map(|di| di.path())
        .map(|path| {
            (
                path.file_stem()
                    .and_then(|fstem| fstem.to_str())
                    .map(|s| s.to_string()),
                path,
            )
        })
        .flat_map(|(fstem, path)| {
            let udid = fstem?;

            let Ok(pf) = PairingFile::read_from_file(path) else {
                return None;
            };

            Some((udid, pf))
        })
        .collect()
}
