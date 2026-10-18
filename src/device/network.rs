use std::net::{IpAddr, SocketAddr, SocketAddrV6};

use idevice::{
    IdeviceService, heartbeat::HeartbeatClient, pairing_file::PairingFile, provider::TcpProvider,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::{
    conn::NetworkDeviceConn,
    device::{core::DeviceCore, power_assertion::PowerAssertion},
    error::RusbmuxError,
};

#[derive(Debug)]
pub struct NetworkDevice {
    core: DeviceCore,

    addr: IpAddr,

    /// In which interface this device got discovered (only useful for IPv6)
    scope_id: Option<u32>,

    pub mac_address: String,
    service_name: String,

    pub udid: Option<String>,

    hb_handler: JoinHandle<()>,
    _power_assertion: PowerAssertion,
}

impl Drop for NetworkDevice {
    fn drop(&mut self) {
        self.hb_handler.abort();

        self.shutdown();
    }
}

impl NetworkDevice {
    pub async fn new(
        id: u64,
        addr: (IpAddr, Option<IpAddr>),
        scope_id: Option<u32>,
        service_name: String,
        pairing_file: PairingFile,
    ) -> Result<Self, RusbmuxError> {
        let udid = pairing_file.udid.clone();
        let mac_address = pairing_file.wifi_mac_address.clone();

        let (mut heartbeat_client, addr) =
            Self::connect_heartbeat_client(addr, scope_id, pairing_file.clone()).await?;
        let power_assertion = PowerAssertion::new(addr, scope_id, pairing_file)?;

        let core = DeviceCore::new(id);

        let device_shutdown = core.canceler.clone();
        let hb_handler = tokio::spawn(async move {
            let mut interval = 15;

            let mut failed = false;

            loop {
                interval = match heartbeat_client.get_marco(interval).await {
                    Ok(i) => {
                        failed = false;
                        i + 5
                    }
                    // TODO: break if it's a socket error
                    Err(err) => {
                        if failed {
                            warn!(id, "Heartbeat failed, error: {err}, closing device");
                            device_shutdown.cancel();
                            return;
                        }

                        warn!(id, "Heartbeat failed, error: {err}, retrying");
                        failed = true;
                        interval
                    }
                };
                if let Err(err) = heartbeat_client.send_polo().await {
                    if failed {
                        warn!(id, "Heartbeat failed, error: {err}, closing device");
                        device_shutdown.cancel();
                        return;
                    }

                    warn!(id, "Heartbeat failed, error: {err}, retrying");
                    failed = true;
                }
            }
        });

        Ok(Self {
            core,
            addr,
            scope_id,
            mac_address,
            service_name,
            udid,
            hb_handler,
            _power_assertion: power_assertion,
        })
    }

    async fn connect_heartbeat_client(
        addr: (IpAddr, Option<IpAddr>),
        scope_id: Option<u32>,
        pairing_file: PairingFile,
    ) -> Result<(HeartbeatClient, IpAddr), RusbmuxError> {
        let make_provider = |ip: IpAddr, pairing_file: PairingFile| TcpProvider {
            addr: ip,
            scope_id,
            pairing_file,
            label: "rusbmux_heartbeat_client".to_string(),
        };

        let provider = make_provider(addr.0, pairing_file);

        match HeartbeatClient::connect(&provider).await {
            Ok(client) => Ok((client, addr.0)),
            Err(first_error) => {
                if let Some(second_ip) = addr.1 {
                    debug!("Failed to connect to {}, trying {second_ip}", addr.0);

                    let provider = make_provider(second_ip, provider.pairing_file);

                    match HeartbeatClient::connect(&provider).await {
                        Ok(client) => return Ok((client, second_ip)),
                        Err(_) => return Err(RusbmuxError::Idevice(first_error)),
                    }
                }

                Err(RusbmuxError::Idevice(first_error))
            }
        }
    }

    pub async fn connect(&self, destination_port: u16) -> Result<NetworkDeviceConn, RusbmuxError> {
        debug!(
            device_id = self.core.id,
            destination_port, "Creating new connection"
        );

        let socket = match self.addr {
            IpAddr::V4(_) => SocketAddr::new(self.addr, destination_port),
            IpAddr::V6(ipv6) => SocketAddr::V6(SocketAddrV6::new(
                ipv6,
                destination_port,
                0,
                self.scope_id.unwrap_or(0),
            )),
        };
        NetworkDeviceConn::new(socket, self.core.id, self.core.canceler.clone()).await
    }

    #[inline]
    pub fn shutdown(&self) {
        self.core.canceler.cancel();
    }

    pub fn id(&self) -> u64 {
        self.core.id
    }

    pub fn canceler(&self) -> CancellationToken {
        self.core.canceler.clone()
    }
}

impl NetworkDevice {
    pub(crate) fn create_device_attached(&self) -> plist::Value {
        let mut network_address = [0u8; 128];
        match self.addr {
            IpAddr::V4(ipv4) => {
                #[cfg(unix)]
                {
                    const SOCKADDRV4_SIZE: usize = size_of::<libc::sockaddr_in>();
                    let socket = libc::sockaddr_in {
                        #[cfg(target_os = "macos")]
                        sin_len: SOCKADDRV4_SIZE as _,
                        sin_family: libc::AF_INET as _,
                        sin_port: 0,
                        sin_addr: libc::in_addr {
                            s_addr: u32::from(ipv4).to_be(),
                        },
                        sin_zero: [0; 8],
                    };

                    let socket_bytes = unsafe {
                        std::slice::from_raw_parts(
                            (&socket as *const libc::sockaddr_in).cast::<u8>(),
                            SOCKADDRV4_SIZE,
                        )
                    };

                    network_address[..SOCKADDRV4_SIZE].copy_from_slice(socket_bytes);
                }
                #[cfg(windows)]
                {
                    use windows_sys::Win32::Networking::WinSock;
                    const SOCKADDRV4_SIZE: usize = size_of::<WinSock::SOCKADDR_IN>();
                    let socket = WinSock::SOCKADDR_IN {
                        sin_family: WinSock::AF_INET,
                        sin_port: 0,
                        sin_addr: WinSock::IN_ADDR {
                            S_un: WinSock::IN_ADDR_0 {
                                S_addr: u32::from(ipv4).to_be(),
                            },
                        },
                        sin_zero: [0; 8],
                    };

                    let socket_bytes = unsafe {
                        std::slice::from_raw_parts(
                            (&socket as *const WinSock::SOCKADDR_IN).cast::<u8>(),
                            SOCKADDRV4_SIZE,
                        )
                    };

                    network_address[..SOCKADDRV4_SIZE].copy_from_slice(socket_bytes);
                }
            }
            IpAddr::V6(ipv6) => {
                #[cfg(unix)]
                {
                    const SOCKADDRV6_SIZE: usize = size_of::<libc::sockaddr_in6>();

                    let socket = libc::sockaddr_in6 {
                        #[cfg(target_os = "macos")]
                        sin6_len: SOCKADDRV6_SIZE as _,
                        sin6_family: libc::AF_INET6 as _,
                        sin6_port: 0,
                        sin6_flowinfo: 0,
                        sin6_addr: libc::in6_addr {
                            s6_addr: ipv6.octets(),
                        },
                        sin6_scope_id: self.scope_id.unwrap_or(0),
                    };

                    let socket_bytes = unsafe {
                        std::slice::from_raw_parts(
                            (&socket as *const libc::sockaddr_in6).cast::<u8>(),
                            SOCKADDRV6_SIZE,
                        )
                    };

                    network_address[..SOCKADDRV6_SIZE].copy_from_slice(socket_bytes);
                }
                #[cfg(windows)]
                {
                    use windows_sys::Win32::Networking::WinSock;
                    const SOCKADDRV6_SIZE: usize = size_of::<WinSock::SOCKADDR_IN6>();
                    let socket = WinSock::SOCKADDR_IN6 {
                        sin6_family: WinSock::AF_INET6,
                        sin6_port: 0,
                        sin6_flowinfo: 0,
                        sin6_addr: WinSock::IN6_ADDR {
                            u: WinSock::IN6_ADDR_0 {
                                Byte: ipv6.octets(),
                            },
                        },
                        Anonymous: WinSock::SOCKADDR_IN6_0 {
                            sin6_scope_id: self.scope_id.unwrap_or(0),
                        },
                    };

                    let socket_bytes = unsafe {
                        std::slice::from_raw_parts(
                            (&socket as *const WinSock::SOCKADDR_IN6).cast::<u8>(),
                            SOCKADDRV6_SIZE,
                        )
                    };

                    network_address[..SOCKADDRV6_SIZE].copy_from_slice(socket_bytes);
                }
            }
        }

        plist_macro::plist!({
            "DeviceID": self.core.id,
            "MessageType": "Attached",
            "Properties": {
                "ConnectionType": "Network",
                "DeviceID": self.core.id,

                "EscapedFullServiceName": &self.service_name,
                "InterfaceIndex": self.scope_id.unwrap_or(0),
                "NetworkAddress": network_address.to_vec(),
                "SerialNumber":? self.udid.as_deref()
            }
        })
    }
}
