# surface-dial-overlay

A Wayland overlay for the Microsoft Surface Dial. It listens for dial events broadcast over D-Bus and renders a visual indicator on screen.

## How it works

```
Surface Dial (evdev)
       │
  dialmenu daemon
       │  D-Bus signals (com.dialmenu.Daemon)
       ▼
surface-dial-overlay
       │  tiny-skia rendering
       ▼
  wlr-layer-shell overlay
```

The daemon (a separate project) handles raw evdev input and re-broadcasts events as D-Bus signals. This overlay subscribes to those signals and draws a centered, transparent overlay on your Wayland desktop that auto-hides after two seconds of inactivity.

### Visual states

| Dial action | Overlay |
|---|---|
| Idle | Fully transparent (invisible) |
| Rotating clockwise | Dark ring + green arc growing from 12 o'clock |
| Rotating counter-clockwise | Dark ring + red arc shrinking from 12 o'clock |
| Pressed | Filled blue circle |

## Requirements

- A Wayland compositor with [`wlr-layer-shell`](https://wayland.app/protocols/wlr-layer-shell-unstable-v1) support (Sway, Hyprland, KDE Plasma, GNOME 45+, etc.)
- The `dialmenu` daemon running and broadcasting on the session D-Bus

### D-Bus interface

The overlay connects to:

| Property | Value |
|---|---|
| Service | `com.dialmenu.Daemon` |
| Object path | `/com/dialmenu/Daemon` |
| Interface | `com.dialmenu.Daemon` |

Signals consumed:

| Signal | Arguments | Meaning |
|---|---|---|
| `DialRotated` | `delta: i32` (+1 or −1) | One step of rotation |
| `DialPressed` | — | Button pressed |
| `DialReleased` | — | Button released |

## Building

```sh
cargo build --release
```

## Running

```sh
cargo run --release
```

Enable debug logging with:

```sh
RUST_LOG=debug cargo run --release
```

## Project structure

```
src/
├── main.rs      # Entry point: Wayland init, calloop event loop, D-Bus thread
├── dbus.rs      # zbus proxy and async signal listener
└── overlay.rs   # Wayland layer-shell surface, SCTK delegates, rendering
```

### Threading model

- **Main thread** — `calloop` event loop dispatching both Wayland protocol events and D-Bus channel messages
- **D-Bus thread** — single-threaded `tokio` runtime running the async `zbus` signal listener; forwards events through a `calloop::channel` to the main thread
