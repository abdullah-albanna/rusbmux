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

### Cargo

```fish
cargo install rusbmux
```

### Arch Linux (AUR)

```fish
paru -S rusbmux-git
```

## Running

### Systemd

If installing with **cargo**, copy the service file from the repository into systemd first:

```fish
sudo cp systemd/rusbmux.service /usr/lib/systemd/system/rusbmux.service
sudo systemctl daemon-reload
```

Then enable and start it:

```fish
sudo systemctl enable --now rusbmux
```

### Direct execution

```fish
sudo rusbmux
```

</details>

## Current limitations (for now)?

- Not as battle-tested as **usbmuxd**
- Android / FreeBSD not yet supported
- ARM / 32-bit systems not yet tested

## License

Licensed under either of

 - Apache License, Version 2.0, ([LICENSE-APACHE](https://github.com/abdullah-albanna/rusbmux/blob/main/LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 - MIT license ([LICENSE-MIT](https://github.com/abdullah-albanna/rusbmux/blob/main/LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
