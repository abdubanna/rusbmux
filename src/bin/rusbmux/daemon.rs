use crate::{
    handler::LOCKDOWN_PATH,
    watcher::{watch_network_daemon, watch_usb_daemon},
};
use rusbmux::error::RusbmuxError;
use tracing::{error, info, warn};

#[cfg(unix)]
pub type Listener = tokio::net::UnixListener;
#[cfg(unix)]
pub const LISTENER_PATH: &str = "/var/run/usbmuxd";

#[cfg(windows)]
pub type Listener = tokio::net::TcpListener;
#[cfg(windows)]
pub const LISTENER_PATH: &str = "127.0.0.1:27015";

#[cfg(unix)]
pub type ConnectionStream = tokio::net::UnixStream;
#[cfg(windows)]
pub type ConnectionStream = tokio::net::TcpStream;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn launch_activate_socket(
        name: *const libc::c_char,
        fds: *mut *mut libc::c_int,
        cnt: *mut libc::size_t,
    ) -> libc::c_int;
}

#[cfg(target_os = "macos")]
fn launchd_listener(name: &str) -> Result<Option<Listener>, RusbmuxError> {
    use std::{
        ffi::CString,
        os::{
            fd::{FromRawFd, RawFd},
            unix::net::UnixListener as StdUnixListener,
        },
    };

    let name = CString::new(name).unwrap();

    let mut fds: *mut libc::c_int = std::ptr::null_mut();
    let mut count: libc::size_t = 0;

    let ret = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut count) };

    if ret != 0 || count == 0 {
        return Ok(None);
    }

    let fd: RawFd = unsafe { *fds };

    let listener = unsafe { StdUnixListener::from_raw_fd(fd) };

    listener.set_nonblocking(true)?;

    Ok(Some(Listener::from_std(listener)?))
}

async fn get_listener(socket_path: &str) -> Result<Listener, RusbmuxError> {
    #[cfg(windows)]
    return Ok(Listener::bind(socket_path).await?);

    #[cfg(unix)]
    {
        #[cfg(target_os = "macos")]
        {
            if let Some(l) = launchd_listener("Listeners")? {
                // TODO: if it came from launchd, then don't try to remove it or set permissions
                return Ok(l);
            }
        }

        Ok(Listener::bind(socket_path)?)
    }
}

async fn create_lockdown_dir() -> Result<(), RusbmuxError> {
    tokio::fs::create_dir_all(LOCKDOWN_PATH)
        .await
        .inspect_err(|err| error!(LOCKDOWN_PATH, %err, "Failed to create the lockdown folder"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = tokio::fs::metadata(LOCKDOWN_PATH)
            .await?
            .permissions()
            .mode()
            & 0o7777;

        if perms != 0o701 {
            tracing::debug!(
                old_perms = format!("{perms:o}"),
                "Changing `{LOCKDOWN_PATH}` permissions to 701"
            );

            tokio::fs::set_permissions(LOCKDOWN_PATH, std::fs::Permissions::from_mode(0o701))
                .await?;
        }
    }

    Ok(())
}

pub async fn run(socket: Option<String>) -> Result<(), RusbmuxError> {
    let socket = socket.unwrap_or(LISTENER_PATH.to_string());

    #[cfg(unix)]
    {
        let path = std::path::Path::new(&socket);
        if path.exists() {
            tracing::debug!("Socket file already exists, removing...");

            if let Err(err) = std::fs::remove_file(path) {
                warn!(%err, "Failed to remove existing socket file");
            }
        }
    }

    let listener = get_listener(&socket).await?;
    if let Err(err) = create_lockdown_dir().await {
        error!(%err, "Failed to create lockdown directory");
    }

    #[cfg(unix)]
    {
        tracing::debug!("Setting the `ReuseAddr` socket option");
        if let Err(err) = rustix::net::sockopt::set_socket_reuseaddr(&listener, true) {
            warn!(%err, "Failed to set ReuseAddr socket option");
        }

        // macos shuts the entire process if there's something wrong when reading or writing to the
        // socket, so this stops it
        #[cfg(target_os = "macos")]
        {
            tracing::debug!("Setting the `Nosigpipe` socket option");
            if let Err(err) = rustix::net::sockopt::set_socket_nosigpipe(&listener, true) {
                warn!(%err, "Failed to set Nosigpipe socket option");
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tracing::debug!("Setting the socket permissions to 666");
        if let Err(err) = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o666)) {
            warn!(%err, "Failed to set socket permissions");
        }
    }

    info!("Spawning the device watcher");
    tokio::spawn(watch_usb_daemon());

    info!("Spawning the network watcher");
    tokio::spawn(watch_network_daemon());

    tokio::select! {
        _ = start_accepting(listener) => {}
        _ = tokio::signal::ctrl_c()  => {
            info!("Got a Ctrl+C, closing...");
            cleanup().await;
        }
        _ = wait_shutdown() => {
            cleanup().await;
        }
    };

    // TODO: wait until all connections get their response of closing
    Ok(())
}

pub async fn wait_shutdown() {
    #[cfg(unix)]
    {
        let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            warn!("Failed to register SIGTERM handler");
            return;
        };

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Got a SIGTERM signal, closing...");
            }
        }
    }

    #[cfg(windows)]
    {
        let mut shutdown = match tokio::signal::windows::ctrl_shutdown() {
            Ok(s) => s,
            Err(err) => {
                warn!(%err, "Failed to register ctrl_shutdown handler");
                return;
            }
        };
        let mut cbreak = match tokio::signal::windows::ctrl_break() {
            Ok(s) => s,
            Err(err) => {
                warn!(%err, "Failed to register ctrl_break handler");
                return;
            }
        };
        let mut close = match tokio::signal::windows::ctrl_close() {
            Ok(s) => s,
            Err(err) => {
                warn!(%err, "Failed to register ctrl_close handler");
                return;
            }
        };
        let mut logoff = match tokio::signal::windows::ctrl_logoff() {
            Ok(s) => s,
            Err(err) => {
                warn!(%err, "Failed to register ctrl_logoff handler");
                return;
            }
        };
        tokio::select! {
            _ = shutdown.recv() => {
                info!("Got a shutdown signal, closing...");

            }
            _ = cbreak.recv() => {
                info!("Got a break signal, closing...");
            }

            _ = close.recv() => {
                info!("Got a close signal, closing...");
            }
            _ = logoff.recv() => {
                info!("Got a logoff signal, closing...");
            }
        }
    }
}

pub async fn cleanup() {
    for mut device in crate::watcher::CONNECTED_DEVICES.iter_mut() {
        if let Err(err) = device.shutdown().await {
            error!(id = device.id(), %err, "Failed to shutdown device");
        }
    }
}

pub async fn start_accepting(listener: Listener) {
    loop {
        match listener.accept().await {
            Ok((socket, _)) => {
                info!("New connection");
                tokio::spawn(async move {
                    crate::handler::handle_client(socket).await;
                });
            }
            Err(err) => {
                error!("Unable to accept the unix connection: {err:?}");
                error!("There's no reason to continue running, bye");
                cleanup().await;
                std::process::exit(1);
            }
        }
    }
}
