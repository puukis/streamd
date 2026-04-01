# streamd

`streamd` is a low-latency remote desktop prototype for personal use over LAN or WAN.

The primary target is:

- Arch/Linux server on Wayland with NVIDIA hardware encoding
- macOS client with VideoToolbox decode and Metal presentation

The project is written in Rust and split into three crates:

- `streamd-server`: capture, encode, transport, and input injection
- `streamd-client`: receive, decode, render, and input capture
- `streamd-proto`: shared packet definitions and framing helpers

## What It Does

At a high level:

1. The client opens a QUIC control connection to the server.
2. The client asks the server which displays are available.
3. The client chooses a display and requests a session.
4. The server captures that display.
5. The server encodes frames with NVENC and sends video as QUIC unreliable datagrams on the same connection.
6. The client reassembles datagram fragments, decodes with VideoToolbox, and presents with Metal.
7. Keyboard and mouse input flow back to the server over a dedicated QUIC unidirectional stream.

## Current Platform Status

| Platform | Role | Status |
| --- | --- | --- |
| Linux / Wayland / NVIDIA | Server | Primary path, implemented and locally smoke-tested |
| macOS | Client | Primary path, implemented; real hardware validation should happen on a native Mac |
| Windows / NVIDIA | Server | Implemented, but not the primary path documented here |

Important constraints:

- The client is a macOS client for real video presentation.
- The server is implemented for Linux and Windows.
- The security model is intentionally relaxed for personal use on trusted networks.

## Workspace Layout

```text
streamd/
├── crates/
│   ├── streamd-client/
│   ├── streamd-proto/
│   └── streamd-server/
└── README.md
```

## Architecture Overview

### Transport

All traffic flows over a single QUIC connection from client to server on `UDP/9000`.

| Channel | Direction | Mechanism |
| --- | --- | --- |
| Control (session setup, IDR requests, heartbeats) | bidirectional | QUIC reliable stream |
| Input (keyboard + mouse events) | client → server | QUIC unidirectional stream |
| Video fragments | server → client | QUIC unreliable datagrams |
| Cursor state | server → client | QUIC unreliable datagrams |
| Cursor shape | server → client | QUIC reliable stream (control) |

Because the client initiates the single outbound QUIC connection, **no inbound port
forwarding is needed on the client side**. The server requires only `UDP/9000` to be
reachable from the client. This makes WAN use straightforward — see [Connecting Over WAN](#connecting-over-wan).

### Datagram Framing

Every QUIC unreliable datagram begins with a 1-byte type tag:

| Tag | Value | Payload |
| --- | --- | --- |
| `DATAGRAM_TAG_VIDEO` | `0x02` | 18-byte `VideoPacketHeader` + encoded video data |
| `DATAGRAM_TAG_CURSOR` | `0x01` | bincode-encoded `RemoteCursorState` |

The client's `datagram_dispatch_loop` reads all datagrams on the connection and routes
them by tag to the video reassembler or cursor store respectively.

### Video Path

Server:

- Linux capture uses Wayland `ext-image-copy-capture-v1`.
- Linux prefers DMA-BUF + GBM when possible.
- Linux falls back to shared memory capture when DMA-BUF is unavailable or unsupported.
- Windows capture uses DXGI Desktop Duplication.
- Encoding is done with NVENC.
- Each encoded frame is fragmented into QUIC datagrams sized to the negotiated path MTU (`Connection::max_datagram_size()`), falling back to 1200 bytes if not yet known.

Client:

- Datagram fragments are reassembled by `VideoFrameReassembler` into complete compressed frames.
- VideoToolbox performs hardware decode on macOS.
- Metal presents frames using a zero-copy `CVMetalTextureCache` path.
- Incomplete frames (e.g. caused by packet loss) are evicted and an IDR request is sent.

### Input Path

- The macOS client captures global keyboard and mouse events.
- Input is serialized as layout-independent HID-style events.
- Linux injects input through `/dev/uinput`.
- Windows injects input through `SendInput`.

### Display Selection

The client can query server displays and select one explicitly.

The current client selectors accepted by `--display` are:

- numeric display index
- stable display id
- exact display name
- exact display description

If `--display` is omitted, the first advertised display is used.

## Security Model

This project is designed for a trusted personal network or direct WAN connection
between machines you control. It is not hardened for hostile internet exposure.

Current behavior:

- The server generates a self-signed certificate at runtime.
- The client accepts the server certificate without CA validation.
- All traffic (video, input, control) is encrypted by QUIC/TLS 1.3.

If you expose the server to the internet, anyone who can reach `UDP/9000` can attempt
a connection. You should restrict access at the network/firewall level.

## Requirements

### Server Requirements: Linux / Arch / Wayland / NVIDIA

You need:

- Rust toolchain
- `clang` / `libclang` for bindgen during build
- NVIDIA GPU with NVENC support
- NVIDIA driver stack providing:
  - `libcuda.so`
  - `libnvidia-encode.so`
- NVENC headers installed so `nvEncodeAPI.h` exists at:
  - `/usr/local/include/ffnvcodec/nvEncodeAPI.h`
- Wayland compositor support for:
  - `ext-image-copy-capture-v1`
  - `ext-output-image-capture-source-manager-v1`
- `/dev/uinput` access for input injection
- DRM render node access in `/dev/dri`

Notes:

- The Linux fast path expects a compositor/device combination that can provide single-plane linear `XRGB8888` or `ARGB8888` DMA-BUF buffers.
- If DMA-BUF is not available, the server falls back to SHM capture automatically.
- The pipeline tries to enable `SCHED_FIFO` and CPU affinity on Linux. If that fails, the server continues, but logs a warning.

Useful preflight checks on the server:

```bash
nvidia-smi
ls /usr/local/include/ffnvcodec/nvEncodeAPI.h
ls -l /dev/uinput /dev/dri/renderD* /dev/dri/card*
printf 'WAYLAND_DISPLAY=%s\nXDG_SESSION_TYPE=%s\n' "$WAYLAND_DISPLAY" "$XDG_SESSION_TYPE"
```

### Client Requirements: macOS

You need:

- Rust toolchain
- Xcode Command Line Tools
- A Mac with VideoToolbox and Metal support
- Accessibility permission for global input capture
- Input Monitoring permission for global input capture

Notes:

- The real interactive client path is macOS-only.
- Building the client for macOS should be done on the Mac itself.

## Build

From the repository root:

```bash
cargo build -p streamd-server
cargo build -p streamd-client
```

If you only want fast compile validation:

```bash
cargo check -p streamd-proto
cargo check -p streamd-server
cargo check -p streamd-client
```

## Server Build Notes

The NVENC path is enabled when the build can find `nvEncodeAPI.h`.

The build script checks:

- `NVENC_HEADER_PATH`
- `NVENC_INCLUDE_DIR`
- `NVENC_LIB_DIR`
- `CUDA_PATH`

On Linux, the default expected header location is:

```text
/usr/local/include/ffnvcodec/nvEncodeAPI.h
```

If the headers are missing, the server still compiles, but the NVENC encoder is replaced with a runtime error path.

## Running the Server

By default the server listens on:

```text
0.0.0.0:9000
```

Start it with:

```bash
cargo run -p streamd-server
```

Or bind to a specific address:

```bash
cargo run -p streamd-server -- 192.168.1.50:9000
```

Useful logging:

```bash
RUST_LOG=info cargo run -p streamd-server -- 0.0.0.0:9000
RUST_LOG=debug cargo run -p streamd-server -- 0.0.0.0:9000
```

## Running the Client on macOS

The client defaults to:

```text
127.0.0.1:9000
```

Usage:

```bash
cargo run -p streamd-client -- --help
```

Current CLI:

```text
streamd-client [server_addr] [--display <id|index|name>] [--list-displays]
```

## Real Test: Arch Server → Mac Client

### 1. Start the server on the Arch machine

```bash
RUST_LOG=info cargo run -p streamd-server -- 0.0.0.0:9000
```

### 2. On the Mac, list available displays

```bash
cargo run -p streamd-client -- 192.168.1.50:9000 --list-displays
```

Example output:

```text
[0] wayland:67 HDMI-A-2 1920x1080 (ASUSTek COMPUTER INC VG279Q3A ...)
[1] wayland:68 DP-3 3840x2160 (Samsung Electric Company Odyssey G80SD ...)
```

### 3. Start a session on the desired display

By numeric index:

```bash
cargo run -p streamd-client -- 192.168.1.50:9000 --display 1
```

By stable display id:

```bash
cargo run -p streamd-client -- 192.168.1.50:9000 --display wayland:68
```

If you omit `--display`, the client requests the first display.

### 4. Grant macOS permissions when prompted

The client needs:

- Accessibility
- Input Monitoring

Without those, video may still work, but local keyboard/mouse capture will not.

### 5. Confirm firewall / network reachability

The only requirement is that the client can reach `UDP/9000` on the server.
No inbound port is required on the client.

```
client → server UDP/9000   (QUIC: control + input + video + cursor)
```

If the Mac has a firewall that blocks outbound UDP, make sure the client process
is allowed to connect.

## Connecting Over WAN

Because all traffic runs over the single outbound QUIC connection, connecting
from outside your home network requires only one change to your router: forward
`UDP/9000` to the server machine's LAN IP. No port forwarding is needed on the
client side.

### Step 1 — Forward UDP/9000 on your home router

In your router's port forwarding settings:

| Field | Value |
| --- | --- |
| Protocol | UDP |
| External port | 9000 |
| Internal IP | your server's LAN IP (e.g. `192.168.1.50`) |
| Internal port | 9000 |

### Step 2 — Find your home's public IP

Run on the server machine:

```bash
curl ifconfig.me
```

Or use a DDNS service (DuckDNS, Cloudflare) if your ISP assigns a dynamic IP so
the address stays stable across reconnects.

### Step 3 — Connect from anywhere

```bash
cargo run -p streamd-client -- <your-public-ip>:9000 --display 0
```

Or with a DDNS hostname:

```bash
cargo run -p streamd-client -- myhome.duckdns.org:9000 --display 0
```

That is all. No VPN, no relay, no extra flags.

### WAN vs LAN differences

| Aspect | LAN | WAN |
| --- | --- | --- |
| Port forward required | none | server UDP/9000 only |
| Client port forward | none | none |
| Latency | < 1 ms network RTT | depends on distance (10–100 ms typical) |
| Video fragment size | same (path MTU negotiated by QUIC) | same |
| Connection command | `192.168.1.50:9000` | `<public-ip>:9000` |

## Operational Notes

### Capture Mode

On Linux, the server tries:

1. Wayland DMA-BUF capture
2. Wayland SHM capture fallback

If you see DMA-BUF warnings in logs, the fallback path may still be perfectly usable for testing.

### Input Toggle

The client currently toggles local input capture with `Ctrl+Alt+Delete`.

### Display Enumeration

Display ids are stable within the context of what the server currently advertises, but you should not assume they are globally portable across different machines or compositor restarts.

### IDR Requests

If the client detects that a frame cannot be completed (e.g. too many fragments
lost in a row), it sends a `RequestIdr` control message to the server. The server
responds by forcing a keyframe on the next encode, allowing the decoder to
resync without a full reconnect. This happens automatically.

## Troubleshooting

### `NVENC headers were not found`

Install `nv-codec-headers` and make sure `nvEncodeAPI.h` is visible to the build script.

Expected location on Linux:

```text
/usr/local/include/ffnvcodec/nvEncodeAPI.h
```

### `open /dev/uinput` failed

Your server user does not have permission to inject input.

You need access to:

```text
/dev/uinput
```

### Wayland display enumeration fails

If you get errors about `wl_output` or the image-copy-capture globals:

- make sure you are really in a Wayland session
- make sure the compositor exposes the capture protocols this project uses
- make sure the process is running with the correct Wayland environment

Useful check:

```bash
echo "$XDG_SESSION_TYPE"
echo "$WAYLAND_DISPLAY"
```

### DMA-BUF capture is unavailable

That does not necessarily block testing.

The server should fall back to SHM capture automatically. Expect higher CPU cost, but the stream can still work.

### The client says decode/presentation is only supported on macOS

That is expected. The real presentation path is macOS-only.

`--list-displays` is still useful as a smoke test, but actual interactive playback is intended for a Mac client.

### macOS client build from Linux fails

That is also expected unless your Linux machine has a proper macOS cross-compilation C toolchain.

Build the client on the Mac itself for real testing.

### Video is choppy or freezes over WAN

This usually means packet loss is causing many frames to be dropped for missing
fragments. The client will automatically request IDR keyframes to recover. If
the problem is persistent:

- Check that `UDP/9000` is correctly forwarded on the server's router.
- Check that the client's outbound UDP is not filtered by a corporate firewall.
- Try reducing the target framerate or resolution by modifying the `SessionRequest`
  in `crates/streamd-client/src/transport/control.rs`.

### Version mismatch error on connect

Both client and server must be built from the same source tree. Protocol version
is checked during the handshake and the server will reject connections from
clients built against an older or newer protocol.

## Validation Status

As of the current repository state:

- `cargo build -p streamd-server` passes
- `cargo build -p streamd-client` passes on the development machine as a host build
- `cargo check -p streamd-proto` passes
- local server startup works
- local `streamd-client --list-displays` against the server works and returns real Wayland outputs

What still needs real hardware validation:

- full interactive macOS playback on a native Mac
- end-to-end input behavior on the target Mac
- compositor-specific behavior under your exact Wayland setup
- longer-duration stability and latency under real network conditions
- WAN path validation through a real NAT/router

## Known Tradeoffs

- Security is intentionally permissive for personal/trusted-network deployment.
- The client currently uses a CLI, not a dedicated UI.
- The Linux fast path depends on compositor DMA-BUF behavior.
- The Windows server path exists, but this README is focused on the Linux → macOS flow.
- QUIC datagrams are subject to the connection's congestion window. On a heavily
  congested path, the send rate may be throttled more than raw UDP would be.
  In practice this provides better long-term throughput stability.

## Development

Helpful commands:

```bash
cargo fmt --all
cargo check -p streamd-proto
cargo check -p streamd-server
cargo check -p streamd-client
```

If you are changing protocol types, update both client and server in the same change, because the control protocol version is currently `4`.
