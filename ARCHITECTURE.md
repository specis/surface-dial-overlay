# Architecture

## Class Diagram

```mermaid
classDiagram
    direction TB

    class OverlayState {
        +registry_state: RegistryState
        +output_state: OutputState
        +shm: Shm
        +surface: SurfaceKind
        +pool: SlotPool
        +width: u32
        +height: u32
        +rotation_accum: f32
        +is_pressed: bool
        +last_event: Option~Instant~
        +configured: bool
        +exit: bool
        +config: OverlayConfig
        +handle_dial_event(DialEvent)
        +tick_visibility()
        +draw(QueueHandle)
    }

    class SurfaceKind {
        <<enumeration>>
        Layer(LayerSurface)
        Window(Window)
        +wl_surface() WlSurface
        +commit()
        +name() str
    }

    class DialEvent {
        <<enumeration>>
        Rotated(i32)
        Pressed
        Released
    }

    class OverlayConfig {
        +style: Style
        +timeout_ms: u64
        +size: u32
        +colors: Colors
        +pie_menu: PieMenuConfig
    }

    class Style {
        <<enumeration>>
        Arc
        Fill
        PieMenu
    }

    class Colors {
        +cw: [u8·4]
        +ccw: [u8·4]
        +press: [u8·4]
        +background: [u8·4]
    }

    class PieMenuConfig {
        +sections: Vec~String~
        +selected_color: [u8·4]
        +unselected_color: [u8·4]
        +gap_degrees: f32
        +selection_step: f32
    }

    class CompositorHandler {
        <<interface>>
        +scale_factor_changed()
        +transform_changed()
        +frame()
        +surface_enter()
        +surface_leave()
    }

    class LayerShellHandler {
        <<interface>>
        +configure()
        +closed()
    }

    class WindowHandler {
        <<interface>>
        +request_close()
        +configure()
    }

    class ShmHandler {
        <<interface>>
        +shm_state() Shm
    }

    class OutputHandler {
        <<interface>>
        +output_state() OutputState
        +new_output()
        +update_output()
        +output_destroyed()
    }

    class DialDaemonProxy {
        <<interface>>
        service: com.dialmenu.Daemon
        path: /com/dialmenu/Daemon
        +dial_rotated: Signal~i32~
        +dial_pressed: Signal
        +dial_released: Signal
    }

    OverlayState *-- SurfaceKind        : surface
    OverlayState *-- OverlayConfig      : config
    OverlayState ..>  DialEvent         : handles

    OverlayConfig *-- Style             : style
    OverlayConfig *-- Colors            : colors
    OverlayConfig *-- PieMenuConfig     : pie_menu

    OverlayState ..|> CompositorHandler : implements
    OverlayState ..|> LayerShellHandler : implements
    OverlayState ..|> WindowHandler     : implements
    OverlayState ..|> ShmHandler        : implements
    OverlayState ..|> OutputHandler     : implements

    DialDaemonProxy ..> DialEvent       : produces
```

## Component Diagram

```mermaid
graph TB
    subgraph Hardware
        DIAL[Surface Dial]
    end

    subgraph dialmenu_daemon["dialmenu daemon (separate process)"]
        EVDEV[evdev reader]
    end

    subgraph surface_dial_overlay["surface-dial-overlay"]
        subgraph dbus_thread["D-Bus thread  (tokio current_thread)"]
            PROXY[DialDaemonProxy\nzbus async listener]
        end

        CHAN["calloop::channel\n(thread bridge)"]

        subgraph main_thread["Main thread  (calloop event loop)"]
            STATE[OverlayState\nhandle_dial_event]
            TICK[tick_visibility\n100 ms heartbeat]
            RENDER[render_frame\ntiny-skia]
            BUF[wl_shm buffer\nRGBA→BGRA]
        end
    end

    subgraph Wayland_compositor["Wayland compositor"]
        LAYER[wlr-layer-shell\noverlay surface]
        XDG[xdg-window\nfallback]
    end

    SCREEN[Display]

    DIAL      -->|evdev events|    EVDEV
    EVDEV     -->|D-Bus signals|   PROXY
    PROXY     -->|DialEvent|       CHAN
    CHAN       -->|calloop Msg|    STATE
    STATE      -->|triggers|       RENDER
    TICK       -->|timeout reset|  STATE
    RENDER     -->|pixmap|         BUF
    BUF        -->|attach+commit|  LAYER
    BUF        -.->|fallback|      XDG
    LAYER      -->|composite|      SCREEN
    XDG        -.->|composite|     SCREEN
```

## Sequence Diagram — Dial Rotation Event

```mermaid
sequenceDiagram
    actor Dial as Surface Dial
    participant Daemon as dialmenu daemon
    participant DBus as D-Bus (session bus)
    participant Proxy as DialDaemonProxy<br/>(D-Bus thread)
    participant Chan as calloop::channel
    participant State as OverlayState<br/>(main thread)
    participant Skia as tiny-skia
    participant Wl as Wayland compositor

    Dial  ->>  Daemon : evdev rotation event
    Daemon ->> DBus   : DialRotated(delta)
    DBus  ->>  Proxy  : signal received
    Proxy ->>  Chan   : send(DialEvent::Rotated(delta))
    Chan  ->>  State  : calloop dispatches Msg
    State ->>  State  : rotation_accum += delta\nlast_event = now()
    State ->>  Skia   : render_frame(config, accum, pressed)
    Skia  ->>  State  : pixmap (RGBA)
    State ->>  State  : RGBA → BGRA conversion
    State ->>  Wl     : damage_buffer + attach + commit
    Wl    ->>  Wl     : composites overlay on screen
```

## Sequence Diagram — Visibility Timeout

```mermaid
sequenceDiagram
    participant Loop as calloop event loop
    participant State as OverlayState
    participant Skia as tiny-skia
    participant Wl as Wayland compositor

    loop Every 100 ms
        Loop  ->> State : tick_visibility()
        alt last_event elapsed > timeout_ms
            State ->> State : rotation_accum = 0\nis_pressed = false
            State ->> Skia  : render_frame → transparent
            Skia  ->> State : empty pixmap
            State ->> Wl    : commit transparent buffer
        else still active
            State ->> State : no-op
        end
    end
```

## Rendering Style Dispatch

```mermaid
flowchart TD
    RF[render_frame]
    RF --> PM{style?}

    PM -->|PieMenu| VIS1{rotation ≥ 0.5\nor pressed?}
    VIS1 -->|yes| PIEMENU[draw_pie_menu\nN equal wedges\ndiscrete selection\nconfirm dot on press]
    VIS1 -->|no| CLEAR1[transparent]

    PM -->|Arc| VIS2{pressed?}
    VIS2 -->|yes| PRESS_A[draw_press\nbg circle + inner dot]
    VIS2 -->|no| VIS2B{rotation ≥ 0.5?}
    VIS2B -->|yes| ARC[draw_rotation_arc\ndark ring stroke\n+ coloured arc stroke]
    VIS2B -->|no| CLEAR2[transparent]

    PM -->|Fill| VIS3{pressed?}
    VIS3 -->|yes| PRESS_B[draw_press\nbg circle + inner dot]
    VIS3 -->|no| VIS3B{rotation ≥ 0.5?}
    VIS3B -->|yes| FILL[draw_rotation_fill\ndark bg circle\n+ filled wedge from 12 o'clock]
    VIS3B -->|no| CLEAR3[transparent]
```
