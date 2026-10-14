use std::{net::IpAddr, path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{
    Args, ColorChoice, Parser, Subcommand,
    builder::{
        Styles,
        styling::{AnsiColor, Effects},
    },
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info};

use rusbmux::usbmux::{UsbMuxCommon, UsbMuxMsgType, UsbMuxPacket, UsbMuxRequest, UsbMuxVersion};

use crate::{daemon::ConnectionStream, handler::LOCKDOWN_PATH};

/// rusbmux - a usbmuxd replacement in Rust
#[derive(Parser, Debug)]
#[command(
    name = "rusbmux",
    version,
    about = "A usbmuxd replacement in pure Rust - USB multiplexing for Apple devices",
    long_about = None,
    color = ColorChoice::Auto,
    styles = Styles::styled()
        .header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
        .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
        .literal(AnsiColor::Cyan.on_default())
        .placeholder(AnsiColor::Magenta.on_default())
        .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Red.on_default())
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Override daemon socket path / address.
    /// On Unix: path to Unix socket (default: /var/run/usbmuxd).
    /// On Windows: TCP address (default: 127.0.0.1:27015).
    #[arg(long, global = true, value_name = "SOCKET")]
    pub socket: Option<String>,

    /// Increase verbosity (-v, -vv, -vvv). Respects RUST_LOG env var.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Manually add a network device by IP and UDID
    #[command()]
    AddDevice(AddDeviceArgs),
}

#[derive(Args, Debug, Clone)]
pub struct AddDeviceArgs {
    /// Device IP address (IPv4 or IPv6)
    #[arg(long, value_name = "IP")]
    pub ip: IpAddr,

    /// Device UDID
    #[arg(long, value_name = "UDID")]
    pub udid: String,

    /// Force adding the network device, removing previous if exists
    #[arg(long, short, default_value_t = false)]
    pub force: bool,

    /// Timeout in seconds for daemon reply
    #[arg(long, default_value_t = 10, value_name = "SECS")]
    pub timeout: u64,
}

pub async fn run_add_device(args: AddDeviceArgs, socket: String) -> Result<()> {
    let udid = args.udid.trim();
    if udid.is_empty() {
        bail!("--udid must not be empty");
    }

    info!(
        socket,
        udid,
        ip = %args.ip,
        "Adding manual network device"
    );

    let payload = UsbMuxRequest::AddDevice {
        common: UsbMuxCommon::builder()
            .program_name("rusbmux")
            .client_version_string(env!("CARGO_PKG_VERSION")),
        ip: args.ip,
        udid: udid.to_string(),
        force: args.force,
    }
    .into_plist();

    debug!(
        "Request plist: {}",
        plist_macro::pretty_print_plist(&payload)
    );

    let packet = UsbMuxPacket::encode_from(
        plist_macro::plist_value_to_xml_bytes(&payload),
        UsbMuxVersion::Plist,
        UsbMuxMsgType::MessagePlist,
        1,
    );

    let mut stream = connect(&socket).await?;

    stream
        .write_all(&packet)
        .await
        .context("failed to send AddDevice request")?;

    debug!("Sent AddDevice request (tag=1, {} bytes)", packet.len());

    let timeout = Duration::from_secs(args.timeout);
    let reply = tokio::time::timeout(timeout, UsbMuxPacket::from_reader(&mut stream))
        .await
        .with_context(|| {
            format!(
                "timed out waiting for daemon reply ({}s) - daemon may not have implemented AddDevice response or device is unreachable",
                timeout.as_secs()
            )
        })?
        .context("failed to read daemon reply")?;

    debug!(
        tag = reply.header.tag,
        msg_type = ?reply.header.msg_type,
        version = ?reply.header.version,
        "Received reply"
    );

    let plist = reply
        .payload
        .as_plist()
        .context("daemon replied with non-plist payload")?;

    debug!("Reply plist: {}", plist_macro::pretty_print_plist(plist));

    let number = extract_result_number(plist)?;

    handle_result(number, udid, &args, &socket, plist)
}

async fn connect(socket: &str) -> Result<ConnectionStream> {
    let connect_timeout = Duration::from_secs(5);

    #[cfg(unix)]
    {
        use tokio::net::UnixStream;
        debug!(%socket, "Connecting to Unix socket");
        let conn = tokio::time::timeout(connect_timeout, UnixStream::connect(socket))
            .await
            .with_context(|| format!("timed out connecting to daemon socket '{socket}'"))?
            .map_err(|err| {
                let base = match err.kind() {
                    std::io::ErrorKind::NotFound => {
                        format!("daemon socket not found at '{socket}' - is rusbmux running?")
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        format!("permission denied connecting to '{socket}' - check permissions")
                    }
                    _ => format!("failed to connect to daemon socket '{socket}'"),
                };
                anyhow::Error::from(err).context(base)
            })?;
        Ok(conn)
    }

    #[cfg(windows)]
    {
        use tokio::net::TcpStream;
        debug!(%socket, "Connecting to TCP socket");
        let conn = tokio::time::timeout(connect_timeout, TcpStream::connect(socket))
            .await
            .with_context(|| format!("timed out connecting to daemon at '{socket}'"))?
            .with_context(|| format!("failed to connect to daemon at '{socket}'"))?;
        let _ = conn.set_nodelay(true);
        Ok(conn)
    }
}

fn handle_result(
    number: u16,
    udid: &str,
    args: &AddDeviceArgs,
    socket: &str,
    plist: &plist::Value,
) -> Result<()> {
    match number {
        0 => {
            info!(
                "Added network device: udid={udid} ip={} socket={socket}",
                args.ip
            );
            info!("Device will appear as Network type",);
            Ok(())
        }
        2 => {
            error!("BadDevice / No such file (2) - pairing file not found for UDID '{udid}'");
            error!(
                "Expected at: {:?}",
                Path::new(LOCKDOWN_PATH).join(format!("{udid}.plist"))
            );
            error!(
                "Pair the device over USB first, or copy the pairing file to the lockdown directory"
            );
            std::process::exit(2);
        }
        3 => {
            anyhow::bail!(
                "Connection refused (3) - daemon could not reach device at {}",
                args.ip
            );
            // std::process::exit(3);
        }
        22 => {
            anyhow::bail!("Invalid input (22) - daemon could not parse request/pairing file");
            // std::process::exit(22);
        }
        n => {
            error!(
                "Full reply: {}",
                plist_macro::plist_value_to_xml_string(plist)
            );
            std::process::exit(n as i32);
        }
    }
}

fn extract_result_number(plist: &plist::Value) -> Result<u16> {
    let dict = plist.as_dictionary().context("reply is not a dictionary")?;

    let number_val = dict
        .get("Number")
        .context("reply missing 'Number' field (not a Result?)")?;

    if let Some(n) = number_val.as_unsigned_integer() {
        return Ok(n as u16);
    }
    if let Some(n) = number_val.as_signed_integer() {
        return Ok(n as u16);
    }
    bail!("'Number' field is not an integer: {number_val:?}")
}
