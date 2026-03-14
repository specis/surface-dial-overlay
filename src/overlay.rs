use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    delegate_xdg_window, delegate_xdg_shell,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        xdg::{
            window::{Window, WindowConfigure, WindowHandler},
            XdgShellHandler,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use wayland_client::{
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use crate::DialEvent;

const HIDE_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum SurfaceKind {
    Layer(LayerSurface),
    Window(Window),
}

impl SurfaceKind {
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        match self {
            SurfaceKind::Layer(layer) => layer.wl_surface(),
            SurfaceKind::Window(window) => window.wl_surface(),
        }
    }

    pub fn commit(&self) {
        match self {
            SurfaceKind::Layer(layer) => layer.commit(),
            SurfaceKind::Window(window) => window.commit(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            SurfaceKind::Layer(_) => "layer-shell",
            SurfaceKind::Window(_) => "xdg-window",
        }
    }
}

// ---------------------------------------------------------------------------
// State struct
// ---------------------------------------------------------------------------

pub struct OverlayState {
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub shm: Shm,

    pub surface: SurfaceKind,
    pub pool: SlotPool,

    pub width: u32,
    pub height: u32,

    pub rotation_accum: f32,
    pub is_pressed: bool,
    pub last_event: Option<Instant>,

    pub configured: bool,
    pub exit: bool,
    pub qh: QueueHandle<OverlayState>,
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

impl OverlayState {
    pub fn handle_dial_event(&mut self, event: DialEvent) {
        self.last_event = Some(Instant::now());

        match event {
            DialEvent::Rotated(delta) => {
                self.rotation_accum += delta as f32;
                self.is_pressed = false;
            }
            DialEvent::Pressed => {
                self.is_pressed = true;
            }
            DialEvent::Released => {
                self.is_pressed = false;
            }
        }

        if self.configured {
            let qh = self.qh.clone();
            self.draw(&qh);
        }
    }

    pub fn tick_visibility(&mut self) {
        let stale = self
            .last_event
            .map(|t| t.elapsed() > HIDE_AFTER)
            .unwrap_or(false);

        if stale && (self.rotation_accum != 0.0 || self.is_pressed) {
            self.rotation_accum = 0.0;
            self.is_pressed = false;

            if self.configured {
                let qh = self.qh.clone();
                self.draw(&qh);
            }
        }
    }

    pub fn draw(&mut self, _qh: &QueueHandle<Self>) {
        let w = self.width.max(1);
        let h = self.height.max(1);

        let (buffer, canvas) = match self.pool.create_buffer(
            w as i32,
            h as i32,
            (w * 4) as i32,
            wl_shm::Format::Argb8888,
        ) {
            Ok(b) => b,
            Err(e) => {
                log::error!("SlotPool::create_buffer failed: {e}");
                return;
            }
        };

        let mut pixmap = match Pixmap::new(w, h) {
            Some(p) => p,
            None => {
                log::error!("Pixmap::new failed for {}x{}", w, h);
                return;
            }
        };

        render_frame(&mut pixmap, self.rotation_accum, self.is_pressed);

        // tiny-skia = RGBA bytes
        // wl_shm ARgb8888 on little-endian memory = BGRA byte order
        let src = pixmap.data();
        for (d, s) in canvas.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d[0] = s[2]; // B
            d[1] = s[1]; // G
            d[2] = s[0]; // R
            d[3] = s[3]; // A
        }
        let _ = canvas;

        let wl_surface = self.surface.wl_surface();
        wl_surface.damage_buffer(0, 0, w as i32, h as i32);

        if let Err(e) = buffer.attach_to(wl_surface) {
            log::error!("attach buffer failed on {}: {e}", self.surface.name());
            return;
        }

        self.surface.commit();
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_frame(pixmap: &mut Pixmap, rotation_accum: f32, is_pressed: bool) {
    pixmap.fill(Color::TRANSPARENT);

    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let r = cx.min(cy) * 0.82;

    if is_pressed {
        draw_press(pixmap, cx, cy, r);
    } else if rotation_accum.abs() >= 0.5 {
        draw_rotation(pixmap, cx, cy, r, rotation_accum);
    }
}

fn draw_press(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    let path = PathBuilder::from_circle(cx, cy, r).expect("circle");
    let mut paint = Paint::default();
    paint.set_color_rgba8(80, 140, 255, 200);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);

    let inner = PathBuilder::from_circle(cx, cy, r * 0.45).expect("inner circle");
    paint.set_color_rgba8(160, 200, 255, 230);
    pixmap.fill_path(&inner, &paint, FillRule::Winding, Transform::identity(), None);
}

fn draw_rotation(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, accum: f32) {
    {
        let ring = PathBuilder::from_circle(cx, cy, r).expect("ring");
        let mut paint = Paint::default();
        paint.set_color_rgba8(30, 30, 40, 180);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = r * 0.18;
        pixmap.stroke_path(&ring, &paint, &stroke, Transform::identity(), None);
    }

    let sweep_deg = (accum * 15.0).clamp(-300.0, 300.0);
    let sweep_rad = sweep_deg.to_radians();
    let start_rad = -std::f32::consts::FRAC_PI_2;

    if let Some(path) = build_arc(cx, cy, r * 0.91, start_rad, sweep_rad) {
        let mut paint = Paint::default();
        if accum > 0.0 {
            paint.set_color_rgba8(80, 210, 120, 230);
        } else {
            paint.set_color_rgba8(220, 90, 80, 230);
        }
        paint.anti_alias = true;

        let mut stroke = Stroke::default();
        stroke.width = r * 0.15;
        stroke.line_cap = LineCap::Round;

        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn build_arc(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Option<tiny_skia::Path> {
    if sweep.abs() < 0.001 {
        return None;
    }

    let n = ((sweep.abs() / std::f32::consts::FRAC_PI_2).ceil() as u32).max(1);
    let seg = sweep / n as f32;
    let k = (4.0 / 3.0) * ((seg / 2.0).abs().tan());

    let mut pb = PathBuilder::new();
    let mut angle = start;
    pb.move_to(cx + r * angle.cos(), cy + r * angle.sin());

    for _ in 0..n {
        let next = angle + seg;
        let sign = seg.signum();
        let cp1x = cx + r * (angle.cos() - sign * k * angle.sin());
        let cp1y = cy + r * (angle.sin() + sign * k * angle.cos());
        let cp2x = cx + r * (next.cos() + sign * k * next.sin());
        let cp2y = cy + r * (next.sin() - sign * k * next.cos());

        pb.cubic_to(
            cp1x,
            cp1y,
            cp2x,
            cp2y,
            cx + r * next.cos(),
            cy + r * next.sin(),
        );

        angle = next;
    }

    pb.finish()
}

// ---------------------------------------------------------------------------
// SCTK delegate implementations
// ---------------------------------------------------------------------------

impl CompositorHandler for OverlayState {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for OverlayState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl OutputHandler for OverlayState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for OverlayState {
    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if configure.new_size.0 != 0 {
            self.width = configure.new_size.0;
        }
        if configure.new_size.1 != 0 {
            self.height = configure.new_size.1;
        }

        if !self.configured {
            log::info!("Layer-shell surface configured: {}x{}", self.width, self.height);
            self.configured = true;
            self.draw(qh);
        }
    }

    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        log::info!("Layer-shell surface closed");
        self.exit = true;
    }
}

impl XdgShellHandler for OverlayState {}

impl WindowHandler for OverlayState {
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        log::info!("XDG window close requested");
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        if let Some((w, h)) = configure.new_size {
            if w > 0 {
                self.width = w;
            }
            if h > 0 {
                self.height = h;
            }
        }

        if !self.configured {
            log::info!("XDG window configured: {}x{}", self.width, self.height);
            self.configured = true;
        }

        self.draw(qh);
    }
}

impl ProvidesRegistryState for OverlayState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_compositor!(OverlayState);
delegate_output!(OverlayState);
delegate_shm!(OverlayState);
delegate_layer!(OverlayState);
delegate_xdg_shell!(OverlayState);
delegate_xdg_window!(OverlayState);
delegate_registry!(OverlayState);