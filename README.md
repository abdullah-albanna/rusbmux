# rusbmux

> One protocol. Your runtime.

**rusbmux** is a modern, drop-in replacement for **usbmuxd**, written in pure Rust — designed for portability, flexibility, and precise control over the USB communication layer.

---

## ⚠️ Work in Progress

This project is under active development.  
Some features described below are incomplete or not yet implemented.

---

## Why **rusbmux**?

The traditional usbmuxd model assumes a system-level daemon running in the background.

That works — but it comes with trade-offs:

- Tight coupling to system services and external dependences
- You can’t easily ship a self-contained binary
- Cross-platform distribution becomes painful

**rusbmux** rethinks that model.

It gives you control over how the protocol runs — instead of forcing a single global daemon model.

## What makes it better

**rusbmux** isn’t just a rewrite — it’s a more flexible architecture.

- **Flexible runtime model**  
  Run it as a daemon, embed it with full ownership of the USB interface, run it in shared mode while coexisting with other clients, or run in exclusive mode where a single process locks the device while others wait.

- **Drop-in compatibility**  
  Works with existing tools that expect **usbmuxd** — no rewrites needed.

- **Library-first design**  
  Use it directly inside your own applications.

- **Modern Rust implementation**  
  Safer, more maintainable, and easier to extend.

## Features

- USB communication with Apple devices

- WiFi device connections

- Android support

- Can be embedded

## So what's worse (for now)?

Let’s be honest — this isn’t strictly better in every way _yet_.

- Not as battle-tested as **usbmuxd**

- Incomplete feature coverage  
  Some edge cases and protocol behaviors are still being implemented.

- Around 10% slower  
  Moving big files with AFC is slower
