# surface-dial-overlay

A Wayland overlay for the Microsoft Surface Dial. It listens for dial events broadcast over D-Bus and renders a Windows Fluent-style visual indicator on screen.

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

Four styles are available, selected in the config file.

### `dial` (default) — Windows Fluent experience

Matches the Windows Surface Dial interaction model:

| Gesture | Visual |
|---|---|
| **Rotate** (button up) | Dark Fluent disc + glowing accent-blue arc. A white tip dot marks the current position on the arc. |
| **Press & hold** | Radial menu appears — sections arranged around a dark disc with a Fluent centre hub and subtle outer ring. |
| **Rotate while held** | Selection advances through sections (wraps around). An accent-colour dot tracks the selected section. |
| **Release** | Menu closes immediately. |

### `fill`

A dark background disc with a coloured wedge that grows from 12 o'clock as you rotate. Pressing shows a filled press circle.

### `arc`

Ring stroke with a coloured arc that traces the rotation amount.

### `pie_menu`

Circle always split into sections while rotating. Rotation changes selection continuously without requiring a button press.

## Configuration

Config file location:

```
~/.config/surface-dial-overlay/config.toml
```

Missing keys fall back to defaults. The file does not need to exist.

### Full example

```toml
# Style: "dial" (default), "fill", "arc", "pie_menu"
style = "dial"

# Hide the overlay this many milliseconds after the last event.
timeout_ms = 2000

# Overlay size in pixels.
size = 240

[colors]
# All colours: [red, green, blue, alpha]  (0–255)
cw         = [0, 120, 212, 240]    # accent blue — arc indicator
ccw        = [0, 120, 212, 240]    # same for dial style
press      = [0, 120, 212, 255]    # confirmation dot
background = [28, 28, 28, 230]     # Fluent dark surface

[pie_menu]
# Section labels — also used by the "dial" radial menu.
# Maximum ~7 items recommended.
sections = ["Volume", "Scroll", "Zoom", "Undo"]

# Highlight colour for the selected section.
selected_color   = [255, 255, 255, 215]

# Colour for unselected sections.
unselected_color = [255, 255, 255, 32]

# Gap between sections in degrees.
gap_degrees = 3.0

# Raw rotation delta to advance one section. Lower = more sensitive.
selection_step = 5.0
```

## Requirements

- A Wayland compositor with [`wlr-layer-shell`](https://wayland.app/protocols/wlr-layer-shell-unstable-v1) support (Sway, Hyprland, KDE Plasma, GNOME 45+, etc.)
- The `dialmenu` daemon running and broadcasting on the session D-Bus

If the compositor does not support `wlr-layer-shell` the overlay falls back to a standard XDG window.

### D-Bus interface

| Property | Value |
|---|---|
| Service | `com.dialmenu.Daemon` |
| Object path | `/com/dialmenu/Daemon` |
| Interface | `com.dialmenu.Daemon` |

Signals consumed:

| Signal | Arguments | Meaning |
|---|---|---|
| `DialRotated` | `delta: i32` | One step of rotation |
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

Enable debug logging:

```sh
RUST_LOG=debug cargo run --release
```

## Project structure

```
src/
├── main.rs      # Entry point: Wayland init, calloop event loop, D-Bus thread
├── config.rs    # TOML config loading, style/colour/menu types
├── dbus.rs      # zbus proxy and async signal listener
└── overlay.rs   # Wayland surface, SCTK delegates, style-dispatched rendering
```

### Threading model

- **Main thread** — `calloop` event loop dispatching Wayland protocol events and D-Bus channel messages; 100 ms heartbeat for visibility timeout
- **D-Bus thread** — single-threaded `tokio` runtime running the async `zbus` signal listener; forwards events through a `calloop::channel` to the main thread

### Arc rendering

Circular arcs are approximated using cubic Bézier curves, split into at most 90° segments. The control-point distance factor is:

```
k = (4/3) · tan(α/4)
```

where `α` is the sweep angle of one segment. This gives a maximum radial error of ~0.06% of the radius — indistinguishable from a true circle at any overlay size.
