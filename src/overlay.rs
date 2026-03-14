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
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use tiny_skia::{BlendMode, Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use wayland_client::{
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use crate::config::{OverlayConfig, Style};
use crate::DialEvent;

// ---------------------------------------------------------------------------
// Surface wrapper
// ---------------------------------------------------------------------------

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

    pub config: OverlayConfig,
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
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let stale = self
            .last_event
            .map(|t| t.elapsed() > timeout)
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

        render_frame(
            &mut pixmap,
            &self.config,
            self.rotation_accum,
            self.is_pressed,
        );

        // tiny-skia = RGBA bytes
        // wl_shm ARgb8888 on little-endian = BGRA byte order
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
// Rendering — top-level dispatch
// ---------------------------------------------------------------------------

fn render_frame(
    pixmap: &mut Pixmap,
    config: &OverlayConfig,
    rotation_accum: f32,
    is_pressed: bool,
) {
    pixmap.fill(Color::TRANSPARENT);

    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let r = cx.min(cy) * 0.82;

    match &config.style {
        Style::PieMenu => {
            // Pie menu is always shown on rotation or press.
            if rotation_accum.abs() >= 0.5 || is_pressed {
                draw_pie_menu(pixmap, cx, cy, r, rotation_accum, is_pressed, config);
            }
        }
        Style::Arc => {
            if is_pressed {
                draw_press(pixmap, cx, cy, r, &config.colors);
            } else if rotation_accum.abs() >= 0.5 {
                draw_rotation_arc(pixmap, cx, cy, r, rotation_accum, &config.colors);
            }
        }
        Style::Fill => {
            if is_pressed {
                draw_press(pixmap, cx, cy, r, &config.colors);
            } else if rotation_accum.abs() >= 0.5 {
                draw_rotation_fill(pixmap, cx, cy, r, rotation_accum, &config.colors);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared: press indicator
// ---------------------------------------------------------------------------

fn draw_press(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, colors: &crate::config::Colors) {
    let p = &colors.press;
    let bg = &colors.background;

    let path = PathBuilder::from_circle(cx, cy, r).expect("circle");
    let mut paint = Paint::default();
    paint.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);

    let inner = PathBuilder::from_circle(cx, cy, r * 0.45).expect("inner circle");
    paint.set_color_rgba8(p[0], p[1], p[2], p[3]);
    pixmap.fill_path(&inner, &paint, FillRule::Winding, Transform::identity(), None);
}

// ---------------------------------------------------------------------------
// Arc style (original)
// ---------------------------------------------------------------------------

fn draw_rotation_arc(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    accum: f32,
    colors: &crate::config::Colors,
) {
    // Background ring
    {
        let bg = &colors.background;
        let ring = PathBuilder::from_circle(cx, cy, r).expect("ring");
        let mut paint = Paint::default();
        paint.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = r * 0.18;
        pixmap.stroke_path(&ring, &paint, &stroke, Transform::identity(), None);
    }

    let sweep_deg = (accum * 15.0).clamp(-300.0, 300.0);
    let sweep_rad = sweep_deg.to_radians();
    let start_rad = -std::f32::consts::FRAC_PI_2;

    if let Some(path) = build_arc(cx, cy, r * 0.91, start_rad, sweep_rad) {
        let color = if accum > 0.0 { &colors.cw } else { &colors.ccw };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;

        let mut stroke = Stroke::default();
        stroke.width = r * 0.15;
        stroke.line_cap = LineCap::Round;

        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

// ---------------------------------------------------------------------------
// Fill style (default) — filled wedge grows from 12 o'clock
// ---------------------------------------------------------------------------

fn draw_rotation_fill(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    accum: f32,
    colors: &crate::config::Colors,
) {
    // Background circle
    {
        let bg = &colors.background;
        let path = PathBuilder::from_circle(cx, cy, r).expect("bg circle");
        let mut paint = Paint::default();
        paint.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }

    // Filled wedge from 12 o'clock
    let sweep_deg = (accum * 15.0).clamp(-360.0, 360.0);
    let sweep_rad = sweep_deg.to_radians();
    let start_rad = -std::f32::consts::FRAC_PI_2;

    if let Some(path) = build_pie_slice(cx, cy, r, start_rad, sweep_rad) {
        let color = if accum > 0.0 { &colors.cw } else { &colors.ccw };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

// ---------------------------------------------------------------------------
// Pie-menu style
// ---------------------------------------------------------------------------

fn draw_pie_menu(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    accum: f32,
    is_pressed: bool,
    config: &OverlayConfig,
) {
    let pm = &config.pie_menu;
    let n = pm.sections.len();
    if n == 0 {
        return;
    }

    // Determine selected section from accumulated rotation.
    let selected = {
        let idx = (accum / pm.selection_step).floor() as i32;
        idx.rem_euclid(n as i32) as usize
    };

    let section_deg = 360.0_f32 / n as f32;
    let gap = pm.gap_degrees;

    for i in 0..n {
        // Start from top (−90°), each section spans section_deg with a gap.
        let start_deg = i as f32 * section_deg - 90.0 + gap / 2.0;
        let sweep_deg = section_deg - gap;
        if sweep_deg <= 0.0 {
            continue;
        }

        let color = if i == selected {
            &pm.selected_color
        } else {
            &pm.unselected_color
        };

        if let Some(path) =
            build_pie_slice(cx, cy, r, start_deg.to_radians(), sweep_deg.to_radians())
        {
            let mut paint = Paint::default();
            paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
            paint.anti_alias = true;
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // Donut hole — erase the centre so sections appear as a ring.
    if let Some(inner) = PathBuilder::from_circle(cx, cy, r * 0.35) {
        let mut paint = Paint::default();
        paint.blend_mode = BlendMode::Clear;
        paint.anti_alias = true;
        pixmap.fill_path(&inner, &paint, FillRule::Winding, Transform::identity(), None);
    }

    // Confirm dot — shown while the button is held.
    if is_pressed {
        if let Some(dot) = PathBuilder::from_circle(cx, cy, r * 0.18) {
            let c = &config.colors.press;
            let mut paint = Paint::default();
            paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
            paint.anti_alias = true;
            pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Build an open arc (stroke path) on the circle at (cx, cy) with radius r,
/// starting at `start` radians and sweeping by `sweep` radians.
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

        pb.cubic_to(cp1x, cp1y, cp2x, cp2y, cx + r * next.cos(), cy + r * next.sin());

        angle = next;
    }

    pb.finish()
}

/// Build a closed pie-slice (filled wedge) from the centre to the arc edge.
fn build_pie_slice(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Option<tiny_skia::Path> {
    if sweep.abs() < 0.001 {
        return None;
    }

    let n = ((sweep.abs() / std::f32::consts::FRAC_PI_2).ceil() as u32).max(1);
    let seg = sweep / n as f32;
    let k = (4.0 / 3.0) * ((seg / 2.0).abs().tan());

    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy);
    pb.line_to(cx + r * start.cos(), cy + r * start.sin());

    let mut angle = start;
    for _ in 0..n {
        let next = angle + seg;
        let sign = seg.signum();
        let cp1x = cx + r * (angle.cos() - sign * k * angle.sin());
        let cp1y = cy + r * (angle.sin() + sign * k * angle.cos());
        let cp2x = cx + r * (next.cos() + sign * k * next.sin());
        let cp2y = cy + r * (next.sin() - sign * k * next.cos());

        pb.cubic_to(cp1x, cp1y, cp2x, cp2y, cx + r * next.cos(), cy + r * next.sin());

        angle = next;
    }

    pb.close();
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
        let (w, h) = configure.new_size;
        if let Some(w) = w {
            self.width = w.get();
        }
        if let Some(h) = h {
            self.height = h.get();
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
