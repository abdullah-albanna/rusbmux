### Core

- [x] Protocol framing (encode/decode usbmux packets)
- [x] Clean error handling
- [x] Logging + debug mode

### Device Management

- [x] Track connected devices
- [x] Handle device unplug safely
- [x] Support multiple devices at once

### Connection

- [x] Raw USB packet parser
- [x] Per-connection state (sequence numbers, etc.)
- [x] Multiplex multiple connections
- [x] Clean connection shutdown
- [ ] Timeout handling
- [x] Respect the device window size

### Runtime Models

- [x] Daemon mode (system-style long running service)
- [ ] Exclusive ownership mode (single process fully owns the USB interface)
- [ ] Shared mode (multiple clients coexist and access is multiplexed safely)
- [ ] Exclusive lock with wait queue (one owner at a time, others block until the owner releases)

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
- [x] Connect
- [x] Listen
- [x] ListListeners
- [x] ReadPairRecord
- [x] ReadBUID
- [x] SavePairRecord
- [x] DeletePairRecord

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
