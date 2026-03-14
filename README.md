# surface-dial-overlay

A Wayland overlay for the Microsoft Surface Dial. It listens for dial events broadcast over D-Bus and renders a configurable visual indicator on screen.

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

The daemon (a separate project) handles raw evdev input and re-broadcasts events as D-Bus signals. This overlay subscribes to those signals and draws a centred, transparent overlay on your Wayland desktop that auto-hides after a configurable period of inactivity.

## Visual styles

Three styles are available, selected via the config file:

### `fill` (default)

A dark background circle with a coloured wedge that grows from 12 o'clock as you rotate. Green for clockwise, red for counter-clockwise. Pressing the button shows a filled circle.

### `arc`

The original style — a dark ring stroke with a coloured arc that traces the rotation amount. Pressing the button shows a filled circle.

### `pie_menu`

The circle is split into equal sections (one per configured label). Rotating highlights adjacent sections in sequence, wrapping around. Holding the button shows a small confirm dot in the centre.

### State summary

| Dial action | `fill` / `arc` | `pie_menu` |
|---|---|---|
| Idle | Transparent | Transparent |
| Rotating CW | Green wedge / arc grows | Next section highlighted |
| Rotating CCW | Red wedge / arc shrinks | Previous section highlighted |
| Pressed | Filled press circle | Confirm dot in centre |

## Configuration

The config file is read from:

```
~/.config/surface-dial-overlay/config.toml
```

If the file does not exist the overlay starts with sensible defaults (`fill` style, 2 s timeout, 200 px).

### Full example

```toml
# Style: "fill" (default), "arc", or "pie_menu"
style = "fill"

# Hide the overlay this many milliseconds after the last event.
timeout_ms = 2000

# Overlay size in pixels.
size = 200

[colors]
# All colours are [red, green, blue, alpha] with values 0–255.
cw         = [80, 210, 120, 230]   # clockwise rotation
ccw        = [220, 90,  80,  230]  # counter-clockwise rotation
press      = [80, 140, 255, 200]   # button press indicator
background = [30, 30,  40,  180]   # background circle / ring

[pie_menu]
# One entry per section. The number of entries sets the section count.
sections = ["Volume", "Brightness", "Scroll", "Zoom"]

# Colour of the currently highlighted section.
selected_color   = [80, 140, 255, 230]

# Colour of all other sections.
unselected_color = [50, 50,  60,  180]

# Visual gap between sections in degrees.
gap_degrees = 4.0

# Raw rotation delta units needed to advance one section.
# Lower = more sensitive. Default: 5.0
selection_step = 5.0
```

Any key not present in the file falls back to the default value, so a minimal config only needs to set what you want to override.

## Requirements

- A Wayland compositor with [`wlr-layer-shell`](https://wayland.app/protocols/wlr-layer-shell-unstable-v1) support (Sway, Hyprland, KDE Plasma, GNOME 45+, etc.)
- The `dialmenu` daemon running and broadcasting on the session D-Bus

If the compositor does not support `wlr-layer-shell` the overlay falls back to a standard XDG window so the program can still run.

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
├── config.rs    # TOML config loading and style/colour types
├── dbus.rs      # zbus proxy and async signal listener
└── overlay.rs   # Wayland surface, SCTK delegates, style-dispatched rendering
```

### Threading model

- **Main thread** — `calloop` event loop dispatching both Wayland protocol events and D-Bus channel messages
- **D-Bus thread** — single-threaded `tokio` runtime running the async `zbus` signal listener; forwards events through a `calloop::channel` to the main thread
