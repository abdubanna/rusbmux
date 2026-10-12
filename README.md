# rusbmux

**rusbmux** is a modern, drop-in replacement for **usbmuxd**, written in pure Rust.

## Why use this?

- Works with your existing tools - [libimobiledevice](https://github.com/libimobiledevice/libimobiledevice), [idevice](https://github.com/jkcoxson/idevice), [3uTools](https://3u.com), [iTunes](https://www.apple.com/itunes), etc. work without changes
- Runs on Windows, macOS and Linux
- WiFi support - connects to devices on your network automatically
- Safer & faster - pure Rust, no C dependencies (except Windows)

## Installation

<details>
<summary><strong>Linux</strong></summary>

### Releases

Download the latest release from the [releases](https://github.com/abdubanna/rusbmux/releases/latest).

### Cargo

```fish
cargo install rusbmux
```

### Arch Linux (AUR)

```fish
paru -S rusbmux-git
```

or

```fish
paru -S rusbmux-bin
```

---

## Running

### Systemd

If you installed `rusbmux` with **Cargo**, install the service file first:

```fish
sudo cp systemd/rusbmux.service /usr/lib/systemd/system/rusbmux.service
sudo systemctl daemon-reload
```

Enable and start the service:

```fish
sudo systemctl enable --now rusbmux
```

### Direct execution

```fish
sudo rusbmux
```

</details>

<details>
<summary><strong>Windows</strong></summary>

### Releases

Download the latest installer from the [releases](https://github.com/abdubanna/rusbmux/releases/latest).

Run the installer and follow the setup wizard. `rusbmux` will be installed as a Windows service and start automatically.

---

## iTunes Compatibility

If you have removed **Apple Mobile Device Support** from your system, iTunes will not be able to communicate with iOS devices.

To restore compatibility, install or restore the Apple Mobile Device Support files.

### Required directories

```text
C:\Program Files\Common Files\Apple\Mobile Device Support\
C:\Program Files (x86)\Common Files\Apple\Mobile Device Support\
```

These directories should contain Apple's Mobile Device Support libraries

### Required registry entries

If the registry entries have also been removed, recreate them with the following `.reg` file:

```reg
Windows Registry Editor Version 5.00

[HKEY_LOCAL_MACHINE\SOFTWARE\Apple Inc.]

[HKEY_LOCAL_MACHINE\SOFTWARE\Apple Inc.\Apple Mobile Device Support]
"InstallDir"="C:\\Program Files\\Common Files\\Apple\\Mobile Device Support\\"
"Version"="18.0.0.32"

[HKEY_LOCAL_MACHINE\SOFTWARE\Apple Inc.\Apple Mobile Device Support\Shared]
"AirTrafficHostDLL"="C:\\Program Files\\Common Files\\Apple\\Mobile Device Support\\AirTrafficHost.dll"
"MobileDeviceDLL"="C:\\Program Files\\Common Files\\Apple\\Mobile Device Support\\MobileDevice.dll"

[HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Apple Inc.]

[HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Apple Inc.\Apple Mobile Device Support]

[HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Apple Inc.\Apple Mobile Device Support\Shared]
"ASMapiInterfaceDLL"="C:\\Program Files (x86)\\Common Files\\Apple\\Mobile Device Support\\AppleSyncMapiInterface.dll"
```

> **Note**
>
> `rusbmux` does not require iTunes or Apple Mobile Device Support to function. These files and registry entries are only required if you also want to use iTunes.

</details>

<details>
<summary><strong>macOS</strong></summary>

### Releases

macOS already ships with Apple's `usbmuxd`, so installing `rusbmux` is optional.

If you'd like to use `rusbmux` instead, download the latest installer from the [releases](https://github.com/abdubanna/rusbmux/releases/latest).

---

## Running

Before starting `rusbmux`, disable Apple's `usbmuxd` service:

```fish
sudo launchctl bootout system /Library/Apple/System/Library/LaunchDaemons/com.apple.usbmuxd.plist
sudo launchctl disable system/com.apple.usbmuxd
```

Then start `rusbmux`:

```fish
sudo launchctl enable system/com.abdubanna.rusbmux
sudo launchctl bootstrap system /Library/LaunchDaemons/com.abdubanna.rusbmux.plist
```

## Switching back to Apple's `usbmuxd`

Disable `rusbmux`

```fish
sudo launchctl bootout system /Library/LaunchDaemons/com.abdubanna.rusbmux.plist
sudo launchctl disable system/com.abdubanna.rusbmux
```

Then re-enable Apple's service:

```fish
sudo launchctl enable system/com.apple.usbmuxd
sudo launchctl bootstrap system /Library/Apple/System/Library/LaunchDaemons/com.apple.usbmuxd.plist
```

You can switch between `rusbmux` and Apple's `usbmuxd` at any time by stopping one and starting the other.

</details>

<details>
<summary><strong>Nix</strong></summary>

### Nix-flake

- Add this to `flake.nix`:

  ```nix
  inputs = {
    rusbmux.url = "github:abdubanna/rusbmux"; # or to specify `github:abdubanna/rusbmux/v0.2.1`
  };
  ```

- In `configuration.nix` (must receive `inputs`):

  ```nix
  imports = [ inputs.rusbmux.nixosModules.default ];
  services.rusbmux.enable = true;
  ```

- That's it: the module applies its own overlay, so `pkgs.rusbmux` is also
  available anywhere in your config (`environment.systemPackages = [ pkgs.rusbmux ];`).

### NixOS Module

- Fetch the repo in `configuration.nix` and wire overlay + module:

  ```nix
  let
    rusbmux-src = builtins.fetchGit {
      url = "https://github.com/abdubanna/rusbmux";
      ref = "main";
    };
  in {
    imports = [ "${rusbmux-src}/nixos-module.nix" ];
    nixpkgs.overlays = [ (import "${rusbmux-src}/overlay.nix") ];
    services.rusbmux.enable = true;
  }
  ```

- Or just build the package: `import "${rusbmux-src}/default.nix" { inherit pkgs; }`
- Pin to a specific commit for reproducibility:

  ```nix
  rusbmux-src = builtins.fetchGit {
    url = "https://github.com/abdubanna/rusbmux";
    rev = "<commit-sha>";
  };
  ```

- Needs nixpkgs with **Rust ≥ 1.88** (edition 2024) — use a recent
  `nixos-unstable`/`nixos-25.05+`.

#### Options

| Option      | Type            | Default        | Description                    |
| ----------- | --------------- | -------------- | ------------------------------ |
| `enable`    | boolean         | `false`        | Enable the systemd service.    |
| `package`   | package         | `pkgs.rusbmux` | Which rusbmux package to run.  |
| `extraArgs` | list of strings | `[]`           | Extra CLI args for the daemon. |

```nix
services.rusbmux = {
  enable = true;
  extraArgs = [ "--socket" "/var/run/rusbmux" ];
};
```

### Nix (Manual use)

- `nix run github:abdubanna/rusbmux -- --help`
- `nix shell github:abdubanna/rusbmux`

</details>

## Using rusbmux as a library

`rusbmux` is a daemon, but it can also be used as a **library** to talk to an
Apple device over USB directly from your own app — no Unix socket, no daemon
process.

It works by exposing rusbmux's direct USB connection as an
[`idevice`](https://github.com/jkcoxson/idevice) connection provider.
[`RusbmuxProvider`](https://docs.rs/rusbmux/latest/rusbmux/provider/struct.RusbmuxProvider.html)
wraps a `UsbDeviceShared` and implements `idevice::provider::IdeviceProvider`.

`idevice` — the library, its service clients talks
to a device through that one trait. Because `RusbmuxProvider` implements it,
every service client that takes a provider (`LockdownClient`,
`NotificationProxyClient`, `AfcClient`, …) can be pointed at a USB device
through rusbmux, no `usbmuxd` needed.

```
your app ── LockdownClient::connect(&provider)
                │  idevice::provider::IdeviceProvider
                ▼
        RusbmuxProvider
                │  bridges a UsbDeviceConn into a tokio
                │  AsyncRead + AsyncWrite stream
                ▼
           UsbDevice
```

### Example

Add rusbmux and the `idevice` features you need:

```toml
[dependencies]
rusbmux = { version = "0.2", default-features = false, features = ["nusb"] }
idevice = "0.1.65"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
futures-lite = "2"

```

```rust,no_run
use futures_lite::stream::StreamExt;
use idevice::{IdeviceService, services::lockdown::LockdownClient};
use rusbmux::{
    device::Device,
    provider::RusbmuxProvider,
    watcher::{UsbEvent, watch_usb},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut usb_hotplug = watch_usb(&rusbmux::usb_backend::DEFAULT_BACKEND);

    let UsbEvent::Connected {
        device: Device::Usb(device),
        id,
    } = usb_hotplug.next().await.unwrap().unwrap()
    else {
        return Err("no USB device connected".into());
    };
    println!("[{id}] device connected: {}", device.udid);

    let provider = RusbmuxProvider::new(&device, "rusbmux-example".to_string());

    let mut lockdown = LockdownClient::connect(&provider).await?;
    let name = lockdown.get_value(Some("DeviceName"), None).await?;
    let udid = lockdown.get_value(Some("UniqueDeviceID"), None).await?;

    println!("Name: {name:?}");
    println!("UDID: {udid:?}");
    Ok(())
}
```

### Notes

#### Exclusive USB ownership

Library mode talks to the device over its raw
bulk endpoints, which can only be opened by one process. Stop the `rusbmux`
daemon (or `usbmuxd`) before using the library.

A more flexable modes are planned, including coexisting

#### Pairing

Services that start a TLS session need a pairing file.

`provider::RusbmuxProvider::preflight`
will check and pair and wait for the trust dialog if needed, and
and sets the new pairing file.

Prefer `provider::RusbmuxProvider::set_pairing_file` when you
already have a pairing file.

A better choice is to set the pairing file AND do preflight, if the pairing file
is valid, preflight is skipped, if not, it would generate a new one for you, you can
then check if preflight did generate a new pairing file (by the returned boolean),
and get it and save it somewhere for later.

## Current limitations (for now)?

- Not as battle-tested as **usbmuxd**
- Android / FreeBSD not yet supported

## MSRV

1.88

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](https://github.com/abdubanna/rusbmux/blob/main/LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](https://github.com/abdubanna/rusbmux/blob/main/LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
