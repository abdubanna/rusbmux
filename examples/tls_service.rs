use futures_lite::stream::StreamExt;
use idevice::{IdeviceService, services::syslog_relay::SyslogRelayClient};
use rusbmux::{
    device::Device,
    provider::RusbmuxProvider,
    watcher::{UsbEvent, watch_usb},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let mut usb_hotplug = watch_usb(&rusbmux::usb_backend::DEFAULT_BACKEND);

    let UsbEvent::Connected {
        device: Device::Usb(device),
        id,
    } = usb_hotplug.next().await.unwrap().unwrap()
    else {
        return Err("no USB device connected".into());
    };

    println!("[{id}] device connected: {}", device.udid());

    let mut provider = RusbmuxProvider::new(&device, "rusbmux-example".to_string());

    provider.preflight().await?;

    let mut syslog = SyslogRelayClient::connect(&provider).await?;

    while let Ok(l) = syslog.next().await {
        println!("{l}");
    }

    Ok(())
}
