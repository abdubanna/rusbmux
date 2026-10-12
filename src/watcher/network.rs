use std::{collections::HashMap, net::IpAddr, path::Path};

use futures_lite::Stream;
use idevice::{
    mdns::{decode_auth_tag, derive_auth_tag},
    pairing_file::PairingFile,
};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use tracing::{debug, error, info, warn};

use crate::{
    device::Device,
    error::RusbmuxError,
    handler::LOCKDOWN_PATH,
    usb_backend::next_device_id,
    watcher::{DeviceEvent, get_hotplug_event_tx},
};

#[derive(Debug)]
pub enum NetworkEvent {
    Connected(Device),
    Disconnected(u64),
}

use super::CONNECTED_DEVICES;

pub const SERVICE_TYPE: &str = "_apple-mobdev2._tcp.local.";

// TODO: use the watch_network()
pub async fn watch_network_daemon() {
    let Ok(mdns) = ServiceDaemon::new() else {
        error!("Failed to create mDNS daemon");
        return;
    };

    let Ok(receiver) = mdns.browse(SERVICE_TYPE) else {
        error!("Failed to browse mDNS");
        return;
    };

    // TODO: it should probably retry if recv_async failed
    while let Ok(event) = receiver.recv_async().await {
        match event {
            ServiceEvent::ServiceResolved(rs) => {
                tokio::spawn(network_device_add(rs));
            }
            ServiceEvent::ServiceRemoved(_, name) => {
                let Some(mac_address) = name.split('@').next() else {
                    debug!(
                        service_name = name,
                        "`@` was not found in the removed service name"
                    );
                    continue;
                };

                debug!(mac_address, "Removing network device");

                let Some(id) = crate::utils::find_network_id_by_mac_address(mac_address) else {
                    debug!(mac_address, "Network device is not connected, skipping");
                    continue;
                };

                let Some((_, device)) = CONNECTED_DEVICES.remove(&id) else {
                    warn!(id, "Device was already removed from CONNECTED_DEVICES ?");
                    continue;
                };
                let _ = device.shutdown().await;

                // dedup
                if !crate::utils::has_usb_with_udid(&device.udid()) {
                    let _ = get_hotplug_event_tx()
                        .await
                        .send(DeviceEvent::Detached { id });
                }
            }
            _ => {}
        }
    }
}

pub async fn watch_network() -> impl Stream<Item = Result<NetworkEvent, RusbmuxError>> {
    async_stream::stream! {
        let mdns = ServiceDaemon::new().map_err(|err| {
            RusbmuxError::UnexpectedPacket(format!("Failed to create mDNS daemon: {err}"))
        })?;

        let receiver = mdns.browse(SERVICE_TYPE).map_err(|err| {
            RusbmuxError::UnexpectedPacket(format!("Failed to browse mDNS: {err}"))
        })?;

        let mut devices_id_map = HashMap::new();
        while let Ok(event) = receiver.recv_async().await {
            match event {
                ServiceEvent::ServiceResolved(rs) => {
                    let Some(rd) = resolve_service(rs) else {
                        continue;
                    };

                    let id = next_device_id();

                    let device = match Device::new_network(
                        id,
                        rd.addr,
                        Some(rd.scope_id),
                        rd.mac_address.clone(),
                        rd.service_name,
                        rd.udid
                    ).await {
                        Ok(d) => {d},
                        Err(err) => {
                            yield Err(err);
                            continue
                        }
                    };

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

                    if let Some(id) = devices_id_map.get(mac_address) {
                        yield Ok(NetworkEvent::Disconnected(*id))
                    };
                }
                _ => {}
            }
        }
    }
}

struct ResolvedDevice {
    addr: (IpAddr, Option<IpAddr>),
    scope_id: u32,
    mac_address: String,
    service_name: String,
    udid: String,
}

fn resolve_service(rs: Box<ResolvedService>) -> Option<ResolvedDevice> {
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
        let Some(udid) = find_udid_from_txt(ident, &auth_tags) else {
            warn!("The device doesn't have a pairing file saved, skipping");
            return None;
        };

        udid
    } else {
        // iOS < 26.4 fallback: parse MAC out of the instance name (`<MAC>@<id>.…`).
        let Some(udid) = get_udid_from_mac_addr(mac_address) else {
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

async fn network_device_add(rs: Box<ResolvedService>) {
    let Some(rd) = resolve_service(rs) else {
        return;
    };

    // if the device broadcasted twice, and the first is still connecting to the heartbeat, this
    // would get the device connect twice
    //
    // TODO: check on the resolved service it self
    if crate::utils::has_network_with_udid(&rd.udid) {
        debug!(udid = rd.udid, "Device already added, skipping");
        return;
    }

    let device = match Device::new_network(
        next_device_id(),
        rd.addr,
        Some(rd.scope_id),
        rd.mac_address,
        rd.service_name,
        rd.udid.clone(),
    )
    .await
    {
        Ok(d) => d,
        Err(err) => {
            error!(udid = rd.udid, %err, "Couldn't create a new network device");
            return;
        }
    };

    let id = device.id();
    CONNECTED_DEVICES.insert(id, device);

    // prefer usb devices over network devices for the same udid
    //
    // skip hotplug notifications when the device is already connected via usb
    //
    // it would get notified only if the usb is disconnected
    if !crate::utils::has_usb_with_udid(&rd.udid) {
        let _ = super::get_hotplug_event_tx()
            .await
            .send(super::DeviceEvent::Attached { id });
    }
}

fn find_udid_from_txt(identifier: &[u8], auth_tags: &[&[u8]]) -> Option<String> {
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

    for (udid, PairingFile { host_id, .. }) in get_saved_pairing_files() {
        let expected = derive_auth_tag(host_id.as_bytes(), identifier);
        if decoded_tags.contains(&expected) {
            info!(udid, "TXT record matched UDID");
            return Some(udid);
        }
    }

    None
}

/// gets all the valid pairing files along side it's file stem (udid)
fn get_saved_pairing_files() -> Vec<(String, PairingFile)> {
    let entries = match Path::new(LOCKDOWN_PATH).read_dir() {
        Ok(dirs) => dirs,
        Err(err) => {
            debug!(%err, "Failed to read the directories of `{LOCKDOWN_PATH}`");
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

fn get_udid_from_mac_addr(mac_addr: &str) -> Option<String> {
    for (
        udid,
        PairingFile {
            wifi_mac_address, ..
        },
    ) in get_saved_pairing_files()
    {
        if mac_addr == wifi_mac_address {
            return Some(udid);
        }
    }

    None
}
