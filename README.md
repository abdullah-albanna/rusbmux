## ⚠️ Work in Progress

---

**rusbmux** is a drop-in replacement for **usbmuxd**, written in Rust.

It lets you communicate with Apple devices **without requiring usbmuxd to be installed or running system-wide**.

## Why?

`usbmuxd` must be installed and running as a system daemon to communicate with Apple devices.

That creates friction:

- You can’t easily ship a self-contained binary
- You depend on system services you don’t control
- Cross-platform distribution becomes painful

Sometimes you just want to ship a program that just... works.

## What rusbmux does differently

**rusbmux** removes the daemon dependency while staying compatible with existing tooling:

- Portable
- Can run as a daemon _or_ non-daemon (no sockets)
- Can be Used directly as a library by existing **usbmuxd** clients
- No separate installation step
- Android support

## Roadmap

### Core

- [x] Protocol framing (encode/decode usbmux packets)
- [ ] Clean error handling
- [ ] Logging + debug mode

### Device Management

- [x] Track connected devices
- [ ] Handle device unplug safely
- [ ] Support multiple devices at once

### Connection

- [x] Raw USB packet parser
- [ ] Per-connection state (sequence numbers, etc.)
- [ ] Multiplex multiple connections
- [ ] Clean connection shutdown
- [ ] Timeout handling

### Compatibility

- [ ] Support old and new device protocol versions
- [ ] Test against multiple iOS versions

### Security

- [ ] Safe storage of pair records (on disk/on memory)

### Performance

- [ ] Benchmark
- [ ] Reduce memory allocations as much as possible
- [ ] Optimize packet parsing/encoding/decoding

### Commands

- [x] ListDevices
- [ ] Connect
- [x] Listen
- [x] ListListeners
- [x] ReadPairRecord
- [ ] ReadBUID
- [ ] SavePairRecord
- [ ] DeletePairRecord

### Lib

- [ ] Public Rust API
- [ ] An rusbmux provider for [idevice](https://github.com/jkcoxson/idevice)
- [ ] FFI for other languages

### Platforms

- [x] Linux
- [x] macOS
- [ ] Android
- [ ] FreeBSD
- [ ] Windows (not sure about this)

### Arch

- [x] x86_64
- [ ] 32-bit
- [ ] ARM
