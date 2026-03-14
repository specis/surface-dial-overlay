mod dbus;
mod overlay;

use std::time::Duration;

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use overlay::OverlayState;
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    registry::RegistryState,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell},
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm},
};
use wayland_client::{globals::registry_queue_init, Connection};

#[derive(Debug, Clone)]
pub enum DialEvent {
    Rotated(i32),
    Pressed,
    Released,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // --- Wayland connection and global enumeration ---
    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<OverlayState>(&conn)?;
    let qh = event_queue.handle();

    // --- Bind protocol objects ---
    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;

    // --- Create the layer-shell overlay surface ---
    // Overlay layer: above all normal windows.
    // No anchor (Anchor::empty()) + no margin = compositor centers the surface.
    // KeyboardInteractivity::None: the overlay is purely visual.
    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("surface-dial-overlay"),
        None, // any output
    );
    layer.set_anchor(Anchor::empty());
    layer.set_size(200, 200);
    layer.set_exclusive_zone(-1); // don't push other surfaces aside
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    // Initial commit triggers the compositor's configure roundtrip
    layer.commit();

    // SlotPool: 2 frames worth of 200×200 ARGB8888 pixels
    let pool = SlotPool::new(200 * 200 * 4 * 2, &shm)?;

    // --- calloop event loop ---
    let mut event_loop: EventLoop<OverlayState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // Bridge channel: Sender<DialEvent> goes to the tokio/D-Bus thread;
    // Channel<DialEvent> is registered as a calloop event source here.
    let (dbus_tx, dbus_rx) = calloop::channel::channel::<DialEvent>();

    // Register the Wayland event queue as a calloop source
    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| anyhow::anyhow!("WaylandSource insert failed: {}", e.error))?;

    // Register the D-Bus event channel
    loop_handle
        .insert_source(dbus_rx, |event, _, state| {
            if let calloop::channel::Event::Msg(dial_event) = event {
                state.handle_dial_event(dial_event);
            }
        })
        .map_err(|e| anyhow::anyhow!("channel insert failed: {}", e.error))?;

    // --- Spawn the D-Bus listener on a dedicated tokio thread ---
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(dbus::run(dbus_tx));
    });

    // --- Build initial state ---
    let mut state = OverlayState {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        layer,
        pool,
        width: 200,
        height: 200,
        rotation_accum: 0.0,
        is_pressed: false,
        last_event: None,
        configured: false,
        exit: false,
        qh: qh.clone(),
    };

    // Run the event loop.
    // The idle callback fires after every dispatch batch — used for the
    // auto-hide timer (cheap: just compares Instant, no syscalls).
    event_loop.run(
        Some(Duration::from_millis(100)),
        &mut state,
        |state| {
            state.tick_visibility();
            if state.exit {
                std::process::exit(0);
            }
        },
    )?;

    Ok(())
}
