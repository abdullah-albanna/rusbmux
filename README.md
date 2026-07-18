# rusbmux

**rusbmux** is a modern, drop-in replacement for **usbmuxd**, written in pure Rust.

## Why use this?

- Works with your existing tools - [libimobiledevice](https://github.com/libimobiledevice/libimobiledevice), [idevice](https://github.com/jkcoxson/idevice), [3uTools](https://3u.com), [iTunes](https://www.apple.com/itunes), etc. work without changes
- Runs on Windows, macOS and Linux
- WiFi support - connects to devices on your network automatically
- Safer & faster - pure Rust, no C dependencies

## Installation

<details>
<summary><strong>Linux</strong></summary>

## Installation

### Releases

Download the latest release from the [releases](https://github.com/abdullah-albanna/rusbmux/releases/latest).

### Cargo

```fish
cargo install rusbmux
```

### Arch Linux (AUR)

```fish
paru -S rusbmux-git
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

Download the latest installer from the [releases](https://github.com/abdullah-albanna/rusbmux/releases/latest).

Run the installer and follow the setup wizard. `rusbmux` will be installed as a Windows service and start automatically.

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

## Installation

### Pre-built package

macOS already ships with Apple's `usbmuxd`, so installing `rusbmux` is optional.

If you'd like to use `rusbmux` instead, download the latest installer from the [releases](https://github.com/abdullah-albanna/rusbmux/releases/latest).

---

## Running

Before starting `rusbmux`, disable Apple's `usbmuxd` service:

```fish
sudo launchctl disable system/com.apple.usbmuxd.plist
sudo launchctl bootout system /Library/Apple/System/Library/LaunchDaemons/com.apple.usbmuxd.plist
```

Then start `rusbmux`:

```fish
sudo launchctl enable system/com.abdullah-albanna.rusbmux.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.abdullah-albanna.rusbmux.plist
```

## Switching back to Apple's `usbmuxd`

Disable `rusbmux`

```fish
sudo launchctl bootout system /Library/LaunchDaemons/com.abdullah-albanna.rusbmux.plist
sudo launchctl disable system/com.abdullah-albanna.rusbmux.plist
```

Then re-enable Apple's service:

```fish
sudo launchctl bootstrap system /Library/Apple/System/Library/LaunchDaemons/com.apple.usbmuxd.plist
sudo launchctl enable system/com.apple.usbmuxd.plist
```

You can switch between `rusbmux` and Apple's `usbmuxd` at any time by stopping one and starting the other.

</details>

## Current limitations (for now)?

- Not as battle-tested as **usbmuxd**
- Android / FreeBSD not yet supported

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](https://github.com/abdullah-albanna/rusbmux/blob/main/LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](https://github.com/abdullah-albanna/rusbmux/blob/main/LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
