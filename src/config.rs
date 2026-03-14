use serde::Deserialize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Style
// ---------------------------------------------------------------------------

/// Which visual style to use for the overlay.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// Windows Fluent radial-menu experience (default).
    ///   - Rotate        → arc value indicator
    ///   - Press & hold  → radial menu appears, rotate to select
    ///   - Release       → menu closes
    #[default]
    Dial,
    /// Filled wedge that grows from 12 o'clock as the dial rotates.
    Fill,
    /// Rotating ring with a coloured arc stroke (original behaviour).
    Arc,
    /// Circle always split into sections; rotation changes selection.
    PieMenu,
}

// ---------------------------------------------------------------------------
// Colors — Fluent dark-mode palette by default
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Colors {
    /// Arc / value indicator colour (clockwise) [R, G, B, A].
    /// Default: Windows accent blue #0078D4.
    #[serde(default = "Colors::default_cw")]
    pub cw: [u8; 4],
    /// Arc / value indicator colour (counter-clockwise) [R, G, B, A].
    #[serde(default = "Colors::default_ccw")]
    pub ccw: [u8; 4],
    /// Confirmation / press highlight colour [R, G, B, A].
    #[serde(default = "Colors::default_press")]
    pub press: [u8; 4],
    /// Background disc colour [R, G, B, A].
    /// Default: Fluent dark-mode surface (~#1c1c1c).
    #[serde(default = "Colors::default_background")]
    pub background: [u8; 4],
}

impl Colors {
    // Windows accent blue
    fn default_cw() -> [u8; 4] { [0, 120, 212, 240] }
    fn default_ccw() -> [u8; 4] { [0, 120, 212, 240] }
    fn default_press() -> [u8; 4] { [0, 120, 212, 255] }
    // Fluent dark surface (grey[16] ≈ #292929, with high alpha)
    fn default_background() -> [u8; 4] { [28, 28, 28, 230] }
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            cw: Self::default_cw(),
            ccw: Self::default_ccw(),
            press: Self::default_press(),
            background: Self::default_background(),
        }
    }
}

// ---------------------------------------------------------------------------
// Radial-menu config (used by both `Dial` and `PieMenu` styles)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct PieMenuConfig {
    /// Labels for each section (determines section count; max ~7 recommended).
    #[serde(default = "PieMenuConfig::default_sections")]
    pub sections: Vec<String>,
    /// Highlight colour for the selected section [R, G, B, A].
    /// Default: near-white (Fluent "white" fill).
    #[serde(default = "PieMenuConfig::default_selected_color")]
    pub selected_color: [u8; 4],
    /// Colour for unselected sections [R, G, B, A].
    #[serde(default = "PieMenuConfig::default_unselected_color")]
    pub unselected_color: [u8; 4],
    /// Visual gap between sections in degrees.
    #[serde(default = "PieMenuConfig::default_gap_degrees")]
    pub gap_degrees: f32,
    /// Raw rotation delta units to advance one section.
    /// Lower = more sensitive.
    #[serde(default = "PieMenuConfig::default_selection_step")]
    pub selection_step: f32,
}

impl PieMenuConfig {
    fn default_sections() -> Vec<String> {
        vec![
            "Volume".into(),
            "Scroll".into(),
            "Zoom".into(),
            "Undo".into(),
        ]
    }
    // Fluent white highlight
    fn default_selected_color() -> [u8; 4] { [255, 255, 255, 215] }
    // Fluent subtle white (grey-tinted, low alpha)
    fn default_unselected_color() -> [u8; 4] { [255, 255, 255, 32] }
    fn default_gap_degrees() -> f32 { 3.0 }
    fn default_selection_step() -> f32 { 5.0 }
}

impl Default for PieMenuConfig {
    fn default() -> Self {
        Self {
            sections: Self::default_sections(),
            selected_color: Self::default_selected_color(),
            unselected_color: Self::default_unselected_color(),
            gap_degrees: Self::default_gap_degrees(),
            selection_step: Self::default_selection_step(),
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

fn default_timeout_ms() -> u64 { 2000 }
fn default_size() -> u32 { 240 }

#[derive(Debug, Clone, Deserialize)]
pub struct OverlayConfig {
    /// Visual style (default: `dial`).
    #[serde(default)]
    pub style: Style,
    /// Hide the overlay this many milliseconds after the last event.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Overlay width and height in pixels.
    #[serde(default = "default_size")]
    pub size: u32,
    /// Colour palette.
    #[serde(default)]
    pub colors: Colors,
    /// Radial-menu settings (used by `dial` and `pie_menu` styles).
    #[serde(default)]
    pub pie_menu: PieMenuConfig,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            style: Style::default(),
            timeout_ms: default_timeout_ms(),
            size: default_size(),
            colors: Colors::default(),
            pie_menu: PieMenuConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

pub fn load_config() -> OverlayConfig {
    let path = config_path();
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(cfg) => {
                    log::info!("Loaded config from {:?}", path);
                    return cfg;
                }
                Err(e) => log::warn!("Config parse error at {:?}: {e}, using defaults", path),
            },
            Err(e) => log::warn!("Failed to read config {:?}: {e}, using defaults", path),
        }
    }
    OverlayConfig::default()
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("surface-dial-overlay").join("config.toml")
}
