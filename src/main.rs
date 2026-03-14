mod config;
mod dbus;
mod overlay;

use std::time::Duration;

use anyhow::{anyhow, Result};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use overlay::{OverlayState, SurfaceKind};

use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    registry::RegistryState,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell},
        xdg::{window::WindowDecorations, XdgShell},
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

fn print_globals(globals: &wayland_client::globals::GlobalList) {
    println!("--- Wayland Globals Detected ---");
    globals.contents().with_list(|list| {
        for g in list {
            println!(
                "interface: {:<30} version: {:<2} name: {}",
                g.interface, g.version, g.name
            );
        }
    });
    println!("--------------------------------");
}

fn bind_compositor(
    globals: &wayland_client::globals::GlobalList,
    qh: &wayland_client::QueueHandle<OverlayState>,
) -> Result<CompositorState> {
    CompositorState::bind(globals, qh)
        .map_err(|e| anyhow!("Failed to bind wl_compositor: {e}"))
}

fn bind_shm(
    globals: &wayland_client::globals::GlobalList,
    qh: &wayland_client::QueueHandle<OverlayState>,
) -> Result<Shm> {
    Shm::bind(globals, qh).map_err(|e| anyhow!("Failed to bind wl_shm: {e}"))
}

fn try_bind_layer_shell(
    globals: &wayland_client::globals::GlobalList,
    qh: &wayland_client::QueueHandle<OverlayState>,
) -> Result<LayerShell> {
    LayerShell::bind(globals, qh).map_err(|e| anyhow!("Layer shell unavailable: {e}"))
}

fn try_bind_xdg_shell(
    globals: &wayland_client::globals::GlobalList,
    qh: &wayland_client::QueueHandle<OverlayState>,
) -> Result<XdgShell> {
    XdgShell::bind(globals, qh)
        .map_err(|e| anyhow!("Failed to bind xdg-shell fallback: {e}"))
}

fn main() -> Result<()> {
    env_logger::init();

    println!("Starting surface-dial-overlay...");

    let config = config::load_config();
    let overlay_size = config.size;

    // --- Wayland connection and global enumeration ---
    let conn = Connection::connect_to_env()
        .map_err(|e| anyhow!("Failed to connect to Wayland compositor: {e}"))?;
    let (globals, event_queue) = registry_queue_init::<OverlayState>(&conn)?;
    let qh = event_queue.handle();

    print_globals(&globals);

    // --- Mandatory protocol objects ---
    let compositor = bind_compositor(&globals, &qh)?;
    let shm = bind_shm(&globals, &qh)?;

    // --- Create the base wl_surface ---
    let base_surface = compositor.create_surface(&qh);

    // --- Prefer layer-shell overlay; fall back to xdg-window ---
    let surface = match try_bind_layer_shell(&globals, &qh) {
        Ok(layer_shell) => {
            println!("Layer-shell detected: using overlay mode");

            let layer = layer_shell.create_layer_surface(
                &qh,
                base_surface,
                Layer::Overlay,
                Some("surface-dial-overlay"),
                None,
            );

            layer.set_anchor(Anchor::empty());
            layer.set_size(overlay_size, overlay_size);
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.commit();

            SurfaceKind::Layer(layer)
        }
        Err(e) => {
            println!("{e}");
            println!("Falling back to XDG window mode");

            let xdg = try_bind_xdg_shell(&globals, &qh)?;
            let window = xdg.create_window(base_surface, WindowDecorations::RequestServer, &qh);
            window.set_title("Surface Dial Overlay");
            window.set_app_id("surface-dial-overlay");
            window.commit();

            SurfaceKind::Window(window)
        }
    };

    // --- Shared memory pool ---
    let pool = SlotPool::new((overlay_size * overlay_size * 4 * 2) as usize, &shm)?;

    // --- calloop event loop ---
    let mut event_loop: EventLoop<OverlayState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // Bridge channel: sender goes to the tokio/D-Bus thread,
    // receiver is registered as a calloop source here.
    let (dbus_tx, dbus_rx) = calloop::channel::channel::<DialEvent>();

    // Register Wayland event queue as a calloop source
    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| anyhow!("WaylandSource insert failed: {}", e.error))?;

    // Register D-Bus event channel
    loop_handle
        .insert_source(dbus_rx, |event, _, state| {
            if let calloop::channel::Event::Msg(dial_event) = event {
                state.handle_dial_event(dial_event);
            }
        })
        .map_err(|e| anyhow!("channel insert failed: {}", e.error))?;

    // --- Spawn D-Bus listener on a dedicated tokio thread ---
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(dbus::run(dbus_tx));
    });

    // --- Initial application state ---
    let mut state = OverlayState {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        surface,
        pool,
        width: overlay_size,
        height: overlay_size,
        rotation_accum: 0.0,
        is_pressed: false,
        menu_active: false,
        menu_accum: 0.0,
        last_event: None,
        configured: false,
        exit: false,
        qh: qh.clone(),
        config,
    };

    println!("Event loop started");

    event_loop.run(Some(Duration::from_millis(100)), &mut state, |state| {
        state.tick_visibility();

        if state.exit {
            println!("Exiting...");
            std::process::exit(0);
        }
    })?;

    Ok(())
}