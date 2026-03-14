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
use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform,
};
use wayland_client::{
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use crate::config::{Colors, OverlayConfig, Style};
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
// State
// ---------------------------------------------------------------------------

pub struct OverlayState {
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub shm: Shm,

    pub surface: SurfaceKind,
    pub pool: SlotPool,

    pub width: u32,
    pub height: u32,

    /// Accumulated rotation for arc/fill/pie-menu styles, and for the
    /// `Dial` value-indicator (rotation without a button hold).
    pub rotation_accum: f32,

    /// True while the button is physically held down.
    pub is_pressed: bool,

    /// `Dial` style only: true while the radial menu is open (button held).
    pub menu_active: bool,

    /// `Dial` style only: rotation accumulated while the menu is open.
    /// Reset to 0 each time the menu opens.
    pub menu_accum: f32,

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
        let is_dial = matches!(self.config.style, Style::Dial);

        match event {
            DialEvent::Rotated(delta) => {
                if is_dial && self.menu_active {
                    // Menu is open — rotate selection
                    self.menu_accum += delta as f32;
                } else {
                    // Menu closed (or non-Dial style) — update value arc
                    self.rotation_accum += delta as f32;
                    if !is_dial {
                        // Preserve legacy Arc/Fill/PieMenu behaviour
                        self.is_pressed = false;
                    }
                }
            }
            DialEvent::Pressed => {
                self.is_pressed = true;
                if is_dial {
                    self.menu_active = true;
                    self.menu_accum = 0.0; // fresh selection each press
                }
            }
            DialEvent::Released => {
                self.is_pressed = false;
                self.menu_active = false;
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

        let visible = self.rotation_accum != 0.0 || self.is_pressed || self.menu_active;
        if stale && visible {
            self.rotation_accum = 0.0;
            self.is_pressed = false;
            self.menu_active = false;
            self.menu_accum = 0.0;

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
            self.menu_active,
            self.menu_accum,
        );

        // tiny-skia = RGBA; wl_shm ARgb8888 (little-endian) = BGRA
        let src = pixmap.data();
        for (d, s) in canvas.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
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
// Top-level render dispatch
// ---------------------------------------------------------------------------

fn render_frame(
    pixmap: &mut Pixmap,
    config: &OverlayConfig,
    rotation_accum: f32,
    is_pressed: bool,
    menu_active: bool,
    menu_accum: f32,
) {
    pixmap.fill(Color::TRANSPARENT);

    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let r = cx.min(cy) * 0.90;

    match &config.style {
        Style::Dial => {
            if menu_active {
                draw_dial_menu(pixmap, cx, cy, r, menu_accum, config);
            } else if rotation_accum.abs() >= 0.5 {
                draw_dial_rotation(pixmap, cx, cy, r, rotation_accum, &config.colors);
            }
        }
        Style::Fill => {
            if is_pressed {
                draw_press(pixmap, cx, cy, r, &config.colors);
            } else if rotation_accum.abs() >= 0.5 {
                draw_rotation_fill(pixmap, cx, cy, r, rotation_accum, &config.colors);
            }
        }
        Style::Arc => {
            if is_pressed {
                draw_press(pixmap, cx, cy, r, &config.colors);
            } else if rotation_accum.abs() >= 0.5 {
                draw_rotation_arc(pixmap, cx, cy, r, rotation_accum, &config.colors);
            }
        }
        Style::PieMenu => {
            if rotation_accum.abs() >= 0.5 || is_pressed {
                draw_pie_menu(pixmap, cx, cy, r, rotation_accum, is_pressed, config);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Dial style — Windows Fluent radial menu
// ---------------------------------------------------------------------------

/// Value-adjustment arc shown when rotating without a button hold.
/// Fluent aesthetic: dark disc, glowing accent-blue arc, white tip dot.
fn draw_dial_rotation(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    accum: f32,
    colors: &Colors,
) {
    let bg = colors.background;
    let accent = colors.cw;

    // Dark background disc
    fill_circle(pixmap, cx, cy, r, bg);

    // Subtle track ring — sits at the same radius as the arc, thin enough
    // that the arc clearly sits on top of it.
    stroke_circle(pixmap, cx, cy, r * 0.72, [255, 255, 255, 14], r * 0.022);

    // Arc indicator — three glow passes (wide+faint → narrow+bright).
    // Stroke widths must stay small relative to arc_r so curvature is visible.
    let sweep_deg = (accum * 12.0).clamp(-330.0, 330.0);
    let sweep_rad = sweep_deg.to_radians();
    let start_rad = -std::f32::consts::FRAC_PI_2;
    let arc_r = r * 0.72;
    // core ≈ 4 px, mid ≈ 7 px, outer ≈ 13 px (very transparent)
    for &(width_factor, alpha_factor) in &[(3.2_f32, 0.08_f32), (1.8, 0.22), (1.0, 1.0)] {
        if let Some(path) = build_arc(cx, cy, arc_r, start_rad, sweep_rad) {
            let a = ((accent[3] as f32) * alpha_factor).round() as u8;
            let mut paint = Paint::default();
            paint.set_color_rgba8(accent[0], accent[1], accent[2], a);
            paint.anti_alias = true;
            let mut stroke = Stroke::default();
            stroke.width = r * 0.037 * width_factor;
            stroke.line_cap = LineCap::Round;
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    // Bright white tip dot at the end of the arc
    let end_angle = start_rad + sweep_rad;
    let tip_x = cx + arc_r * end_angle.cos();
    let tip_y = cy + arc_r * end_angle.sin();
    fill_circle(pixmap, tip_x, tip_y, r * 0.07, [255, 255, 255, 235]);

    // Small centre dot
    fill_circle(pixmap, cx, cy, r * 0.055, [255, 255, 255, 160]);
}

/// Radial menu shown while the button is held.
/// Fluent aesthetic: dark disc, white section highlight, subtle outer ring,
/// accent-coloured selection indicator, dark centre hub.
fn draw_dial_menu(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    menu_accum: f32,
    config: &OverlayConfig,
) {
    let pm = &config.pie_menu;
    let n = pm.sections.len().max(1);
    let bg = config.colors.background;
    let accent = config.colors.cw;

    // 1 — Dark background disc
    fill_circle(pixmap, cx, cy, r, bg);

    // 2 — Determine selected section
    let selected = {
        let idx = (menu_accum / pm.selection_step).floor() as i32;
        idx.rem_euclid(n as i32) as usize
    };

    // 3 — Draw sections
    let section_deg = 360.0_f32 / n as f32;
    let gap = pm.gap_degrees;
    let section_outer_r = r * 0.86;

    for i in 0..n {
        let start_deg = i as f32 * section_deg - 90.0 + gap / 2.0;
        let sweep_deg = section_deg - gap;
        if sweep_deg <= 0.0 {
            continue;
        }
        let color = if i == selected {
            pm.selected_color
        } else {
            pm.unselected_color
        };
        if let Some(path) = build_pie_slice(
            cx,
            cy,
            section_outer_r,
            start_deg.to_radians(),
            sweep_deg.to_radians(),
        ) {
            let mut paint = Paint::default();
            paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
            paint.anti_alias = true;
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }

        // Extra highlight ring on selected section outer edge
        if i == selected {
            let mid_deg = i as f32 * section_deg - 90.0 + section_deg / 2.0;
            let mid_rad = mid_deg.to_radians();
            let ind_r = r * 0.91;
            let ix = cx + ind_r * mid_rad.cos();
            let iy = cy + ind_r * mid_rad.sin();
            fill_circle(pixmap, ix, iy, r * 0.045, [accent[0], accent[1], accent[2], 255]);
        }
    }

    // 4 — Subtle outer border ring
    stroke_circle(pixmap, cx, cy, r * 0.95, [255, 255, 255, 38], r * 0.018);

    // 5 — Erase centre hub area (transparent hole)
    let hub_r = r * 0.30;
    if let Some(hub) = PathBuilder::from_circle(cx, cy, hub_r) {
        let mut paint = Paint::default();
        paint.blend_mode = BlendMode::Clear;
        paint.anti_alias = true;
        pixmap.fill_path(&hub, &paint, FillRule::Winding, Transform::identity(), None);
    }

    // 6 — Hub disc (Fluent secondary surface, slightly lighter than bg)
    let hub_col = brighten(bg, 38);
    fill_circle(pixmap, cx, cy, hub_r * 0.88, hub_col);

    // 7 — Tiny accent dot in hub centre
    fill_circle(pixmap, cx, cy, hub_r * 0.22, [accent[0], accent[1], accent[2], 220]);
}

// ---------------------------------------------------------------------------
// Legacy styles (Arc, Fill, PieMenu)
// ---------------------------------------------------------------------------

fn draw_press(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, colors: &Colors) {
    fill_circle(pixmap, cx, cy, r, colors.background);
    let p = colors.press;
    fill_circle(pixmap, cx, cy, r * 0.48, p);
    fill_circle(pixmap, cx, cy, r * 0.22, [255, 255, 255, 200]);
}

fn draw_rotation_arc(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    accum: f32,
    colors: &Colors,
) {
    let bg = colors.background;
    stroke_circle(pixmap, cx, cy, r, bg, r * 0.18);

    let sweep_deg = (accum * 15.0).clamp(-300.0, 300.0);
    let sweep_rad = sweep_deg.to_radians();
    let start_rad = -std::f32::consts::FRAC_PI_2;

    if let Some(path) = build_arc(cx, cy, r * 0.91, start_rad, sweep_rad) {
        let color = if accum > 0.0 { colors.cw } else { colors.ccw };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = r * 0.15;
        stroke.line_cap = LineCap::Round;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn draw_rotation_fill(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    accum: f32,
    colors: &Colors,
) {
    fill_circle(pixmap, cx, cy, r, colors.background);
    let sweep_deg = (accum * 15.0).clamp(-360.0, 360.0);
    let sweep_rad = sweep_deg.to_radians();
    let start_rad = -std::f32::consts::FRAC_PI_2;

    if let Some(path) = build_pie_slice(cx, cy, r, start_rad, sweep_rad) {
        let color = if accum > 0.0 { colors.cw } else { colors.ccw };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

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

    let selected = {
        let idx = (accum / pm.selection_step).floor() as i32;
        idx.rem_euclid(n as i32) as usize
    };

    let section_deg = 360.0_f32 / n as f32;
    let gap = pm.gap_degrees;

    fill_circle(pixmap, cx, cy, r, config.colors.background);

    for i in 0..n {
        let start_deg = i as f32 * section_deg - 90.0 + gap / 2.0;
        let sweep_deg = section_deg - gap;
        if sweep_deg <= 0.0 {
            continue;
        }
        let color = if i == selected {
            pm.selected_color
        } else {
            pm.unselected_color
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

    if let Some(inner) = PathBuilder::from_circle(cx, cy, r * 0.35) {
        let mut paint = Paint::default();
        paint.blend_mode = BlendMode::Clear;
        paint.anti_alias = true;
        pixmap.fill_path(&inner, &paint, FillRule::Winding, Transform::identity(), None);
    }

    if is_pressed {
        let c = config.colors.press;
        fill_circle(pixmap, cx, cy, r * 0.18, c);
    }
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

fn fill_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, color: [u8; 4], width: f32) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = width;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// Brighten an RGBA colour by adding `amount` to each channel, clamped at 255.
fn brighten(c: [u8; 4], amount: u16) -> [u8; 4] {
    [
        (c[0] as u16 + amount).min(255) as u8,
        (c[1] as u16 + amount).min(255) as u8,
        (c[2] as u16 + amount).min(255) as u8,
        c[3],
    ]
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Open arc path — used for stroke rendering.
fn build_arc(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Option<tiny_skia::Path> {
    if sweep.abs() < 0.001 {
        return None;
    }
    let n = ((sweep.abs() / std::f32::consts::FRAC_PI_2).ceil() as u32).max(1);
    let seg = sweep / n as f32;
    // Correct bezier-arc approximation: k = (4/3)·tan(α/4), where α is the
    // segment angle. Using α/2 instead (the previous bug) produces k≈1.33
    // for 90° segments, pushing control points far outside the circle and
    // giving the path a square/rectangular appearance.
    let k = (4.0 / 3.0) * (seg / 4.0).abs().tan();

    let mut pb = PathBuilder::new();
    let mut angle = start;
    pb.move_to(cx + r * angle.cos(), cy + r * angle.sin());

    for _ in 0..n {
        let next = angle + seg;
        let sign = seg.signum();
        pb.cubic_to(
            cx + r * (angle.cos() - sign * k * angle.sin()),
            cy + r * (angle.sin() + sign * k * angle.cos()),
            cx + r * (next.cos() + sign * k * next.sin()),
            cy + r * (next.sin() - sign * k * next.cos()),
            cx + r * next.cos(),
            cy + r * next.sin(),
        );
        angle = next;
    }
    pb.finish()
}

/// Closed pie-slice path — from centre outward; used for fill rendering.
fn build_pie_slice(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Option<tiny_skia::Path> {
    if sweep.abs() < 0.001 {
        return None;
    }
    let n = ((sweep.abs() / std::f32::consts::FRAC_PI_2).ceil() as u32).max(1);
    let seg = sweep / n as f32;
    let k = (4.0 / 3.0) * (seg / 4.0).abs().tan(); // same fix as build_arc

    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy);
    pb.line_to(cx + r * start.cos(), cy + r * start.sin());

    let mut angle = start;
    for _ in 0..n {
        let next = angle + seg;
        let sign = seg.signum();
        pb.cubic_to(
            cx + r * (angle.cos() - sign * k * angle.sin()),
            cy + r * (angle.sin() + sign * k * angle.cos()),
            cx + r * (next.cos() + sign * k * next.sin()),
            cy + r * (next.sin() - sign * k * next.cos()),
            cx + r * next.cos(),
            cy + r * next.sin(),
        );
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
