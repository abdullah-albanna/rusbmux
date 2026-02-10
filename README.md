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
- Android support without root (maybe?)
- Rust

## Roadmap

- [x] ListDevices
- [ ] Connect
- [x] Listen
- [ ] ListListeners
- [ ] ReadPairRecord
- [ ] ReadBUID
- [ ] SavePairRecord
- [ ] DeletePairRecord
