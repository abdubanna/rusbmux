use futures::stream::StreamExt;
use idevice::{IdeviceService, services::lockdown::LockdownClient};
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
        device: Device::Usb(mut device),
        id,
    } = usb_hotplug.next().await.unwrap().unwrap()
    else {
        return Err("no USB device connected".into());
    };
    println!("[{id}] device connected: {:?}", device.udid());

    let provider = RusbmuxProvider::new(&device, "rusbmux-example".to_string(), None);

    let mut lockdown = LockdownClient::connect(&provider).await?;
    let name = lockdown.get_value(Some("DeviceName"), None).await?;
    let udid = lockdown.get_value(Some("UniqueDeviceID"), None).await?;

    device.shutdown().await?;

    println!("Name: {name:?}");
    println!("UDID: {udid:?}");
    Ok(())
}
