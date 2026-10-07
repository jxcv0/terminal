use egui::Color32;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: Font,
    pub window: Window,
    pub colors: Colors,
    pub scrollback_lines: usize,
    pub cursor_blink_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: Font::default(),
            window: Window::default(),
            colors: Colors::default(),
            scrollback_lines: 10_000,
            cursor_blink_ms: 500,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Font {
    pub size: f32,
    pub file: Option<PathBuf>,
    #[serde(skip)]
    pub data: Option<Arc<egui::FontData>>,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            size: 16.0,
            file: None,
            data: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Window {
    pub width: u32,
    pub height: u32,
    pub padding: i8,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            width: 816,
            height: 592,
            padding: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    #[serde(deserialize_with = "deserialize_color")]
    pub foreground: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub background: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub selection: Color32,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            foreground: Color32::from_rgb(221, 225, 231),
            background: Color32::from_rgb(20, 23, 28),
            selection: Color32::from_rgb(54, 76, 106),
        }
    }
}

fn deserialize_color<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Color32, D::Error> {
    let value = String::deserialize(deserializer)?;
    let hex = value.strip_prefix('#').unwrap_or_default();
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(serde::de::Error::custom(
            "expected a color in #RRGGBB format",
        ));
    }
    let rgb = u32::from_str_radix(hex, 16).map_err(serde::de::Error::custom)?;
    Ok(Color32::from_rgb(
        (rgb >> 16) as u8,
        (rgb >> 8) as u8,
        rgb as u8,
    ))
}

fn config_path(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .map(|base| base.join("terminal/config.toml"))
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let path = config_path(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        );
        path.map_or_else(|| Ok(Self::default()), |path| Self::load_file(&path))
    }

    fn parse(source: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(source).map_err(|error| error.to_string())?;
        if !config.font.size.is_finite() || !(4.0..=96.0).contains(&config.font.size) {
            return Err("font.size must be between 4 and 96 points".into());
        }
        if !(64..=16_384).contains(&config.window.width)
            || !(64..=16_384).contains(&config.window.height)
        {
            return Err("window.width and window.height must be between 64 and 16384".into());
        }
        if !(0..=64).contains(&config.window.padding) {
            return Err("window.padding must be between 0 and 64".into());
        }
        if config.scrollback_lines > 1_000_000 {
            return Err("scrollback_lines must be between 0 and 1000000".into());
        }
        if config.cursor_blink_ms > 60_000 {
            return Err("cursor_blink_ms must be between 0 and 60000 (0 disables blinking)".into());
        }
        Ok(config)
    }

    fn load_file(path: &Path) -> Result<Self, String> {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let mut config =
            Self::parse(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        if let Some(file) = &config.font.file {
            let file = path.parent().unwrap_or(Path::new(".")).join(file);
            let load_font = || -> Result<_, String> {
                let bytes = std::fs::read(&file).map_err(|error| error.to_string())?;
                ab_glyph::FontRef::try_from_slice(&bytes).map_err(|error| error.to_string())?;
                Ok(Arc::new(egui::FontData::from_owned(bytes)))
            };
            config.font.data = Some(load_font().map_err(|error| {
                format!("{}: font.file {}: {error}", path.display(), file.display())
            })?);
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_path_precedence_and_fallback() {
        let home = Some(PathBuf::from("/home/test"));
        assert_eq!(
            config_path(Some("/config".into()), home.clone()),
            Some("/config/terminal/config.toml".into())
        );
        for xdg in [None, Some("".into()), Some("relative".into())] {
            assert_eq!(
                config_path(xdg, home.clone()),
                Some("/home/test/.config/terminal/config.toml".into())
            );
        }
        assert_eq!(config_path(None, None), None);
        assert_eq!(config_path(None, Some("relative".into())), None);
        assert_eq!(
            config_path(Some("/config".into()), None),
            Some("/config/terminal/config.toml".into())
        );
    }

    #[test]
    fn empty_partial_and_example_configs_keep_defaults() {
        for source in ["", include_str!("../config.example.toml")] {
            let config = Config::parse(source).unwrap();
            assert_eq!(config.font.size, 16.0);
            assert_eq!(config.window.width, 816);
            assert_eq!(config.window.height, 592);
            assert_eq!(config.window.padding, 8);
            assert_eq!(config.colors.background, Colors::default().background);
            assert_eq!(config.scrollback_lines, 10_000);
            assert_eq!(config.cursor_blink_ms, 500);
        }
        let config = Config::parse("scrollback_lines = 0\ncursor_blink_ms = 0\n[font]\nsize = 20\n[colors]\nforeground = '#aBc123'").unwrap();
        assert_eq!(config.font.size, 20.0);
        assert_eq!(config.colors.foreground, Color32::from_rgb(171, 193, 35));
        assert_eq!(config.colors.selection, Colors::default().selection);
        assert_eq!(config.window.padding, 8);
        assert_eq!(config.scrollback_lines, 0);
        assert_eq!(config.cursor_blink_ms, 0);
    }

    #[test]
    fn malformed_unknown_and_out_of_range_settings_are_rejected() {
        for source in [
            "[font",
            "typo = true",
            "[font]\nszie = 20",
            "[window]\ntypo = 1",
            "[colors]\ntypo = '#112233'",
            "[font]\nsize = 'big'",
            "[font]\nsize = 0",
            "[font]\nsize = 100",
            "[font]\nsize = nan",
            "[font]\nsize = inf",
            "[window]\nwidth = 0",
            "[window]\nheight = 20000",
            "[window]\npadding = -1",
            "[window]\npadding = 65",
            "scrollback_lines = -1",
            "scrollback_lines = 1000001",
            "cursor_blink_ms = -1",
            "cursor_blink_ms = 60001",
            "[colors]\nforeground = '#123'",
            "[colors]\nbackground = '112233'",
            "[colors]\nselection = '#zzzzzz'",
        ] {
            assert!(Config::parse(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn file_loading_reports_errors_and_resolves_relative_fonts() {
        let dir = std::env::temp_dir().join(format!("terminal-config-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("config.toml");
        assert_eq!(Config::load_file(&path).unwrap().font.size, 16.0);
        assert!(
            Config::load_file(&dir)
                .unwrap_err()
                .contains(&dir.display().to_string())
        );
        std::fs::write(&path, "[font]\nfile = 'custom.ttf'\nsize = 22").unwrap();
        let error = Config::load_file(&path).unwrap_err();
        assert!(error.contains("font.file") && error.contains("custom.ttf"));
        std::fs::write(dir.join("custom.ttf"), b"not a font").unwrap();
        assert!(Config::load_file(&path).unwrap_err().contains("font.file"));
        std::fs::write(
            dir.join("custom.ttf"),
            include_bytes!("../assets/fonts/DejaVuSansMono.ttf"),
        )
        .unwrap();
        let config = Config::load_file(&path).unwrap();
        assert_eq!(config.font.size, 22.0);
        assert!(config.font.data.is_some());
        std::fs::write(&path, "[font]\nsize = 0").unwrap();
        assert!(
            Config::load_file(&path)
                .unwrap_err()
                .contains(&path.display().to_string())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
