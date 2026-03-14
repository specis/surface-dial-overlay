use serde::Deserialize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Style
// ---------------------------------------------------------------------------

/// Which visual style to use for the overlay.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// Rotating ring with a coloured arc stroke (original behaviour).
    Arc,
    /// Filled wedge that grows from 12 o'clock as the dial rotates.
    #[default]
    Fill,
    /// Circle split into equal pie sections; rotation changes the selection.
    PieMenu,
}

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Colors {
    /// Clockwise rotation colour [R, G, B, A].
    #[serde(default = "Colors::default_cw")]
    pub cw: [u8; 4],
    /// Counter-clockwise rotation colour [R, G, B, A].
    #[serde(default = "Colors::default_ccw")]
    pub ccw: [u8; 4],
    /// Button-press indicator colour [R, G, B, A].
    #[serde(default = "Colors::default_press")]
    pub press: [u8; 4],
    /// Background ring/circle colour [R, G, B, A].
    #[serde(default = "Colors::default_background")]
    pub background: [u8; 4],
}

impl Colors {
    fn default_cw() -> [u8; 4] { [80, 210, 120, 230] }
    fn default_ccw() -> [u8; 4] { [220, 90, 80, 230] }
    fn default_press() -> [u8; 4] { [80, 140, 255, 200] }
    fn default_background() -> [u8; 4] { [30, 30, 40, 180] }
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
// Pie-menu config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct PieMenuConfig {
    /// Labels for each section (one section per entry).
    #[serde(default = "PieMenuConfig::default_sections")]
    pub sections: Vec<String>,
    /// Colour of the currently selected section [R, G, B, A].
    #[serde(default = "PieMenuConfig::default_selected_color")]
    pub selected_color: [u8; 4],
    /// Colour of unselected sections [R, G, B, A].
    #[serde(default = "PieMenuConfig::default_unselected_color")]
    pub unselected_color: [u8; 4],
    /// Gap between sections in degrees.
    #[serde(default = "PieMenuConfig::default_gap_degrees")]
    pub gap_degrees: f32,
    /// Rotation units (raw delta) needed to advance one section.
    #[serde(default = "PieMenuConfig::default_selection_step")]
    pub selection_step: f32,
}

impl PieMenuConfig {
    fn default_sections() -> Vec<String> {
        vec![
            "Option 1".into(),
            "Option 2".into(),
            "Option 3".into(),
            "Option 4".into(),
        ]
    }
    fn default_selected_color() -> [u8; 4] { [80, 140, 255, 230] }
    fn default_unselected_color() -> [u8; 4] { [50, 50, 60, 180] }
    fn default_gap_degrees() -> f32 { 4.0 }
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
fn default_size() -> u32 { 200 }

#[derive(Debug, Clone, Deserialize)]
pub struct OverlayConfig {
    /// Visual style.
    #[serde(default)]
    pub style: Style,
    /// How long to keep the overlay visible after the last event (ms).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Overlay width and height in pixels.
    #[serde(default = "default_size")]
    pub size: u32,
    /// Colour palette used by arc and fill styles.
    #[serde(default)]
    pub colors: Colors,
    /// Pie-menu specific settings.
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
