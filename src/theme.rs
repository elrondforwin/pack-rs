use std::io;

use crate::config::{Settings, read_toml, write_if_missing};
use ratatui::style::Color;

const DEFAULT_THEME: &str = "[colors]\nselected = 212\ntext = \"gray\"\nmuted = \"darkgray\"\nseparator = 240\nsearch = \"cyan\"\n";

#[derive(Clone, Copy)]
pub struct Theme {
    pub selected: Color,
    pub text: Color,
    pub muted: Color,
    pub separator: Color,
    pub search: Color,
}

impl Theme {
    pub fn load(settings: &Settings) -> io::Result<Self> {
        let themes = settings.directory.join("themes");
        std::fs::create_dir_all(&themes)?;
        write_if_missing(themes.join("default.toml"), DEFAULT_THEME)?;
        let path = themes.join(format!("{}.toml", settings.theme));
        let data = read_toml(&path)?;
        let colors = data
            .get("colors")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                io::Error::other(format!("{}: [colors] bölümü gerekli", path.display()))
            })?;
        let color = |key: &str| -> io::Result<Color> {
            let value = colors.get(key).ok_or_else(|| {
                io::Error::other(format!("{}: colors.{key} gerekli", path.display()))
            })?;
            let raw = value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_integer().map(|number| number.to_string()))
                .ok_or_else(|| {
                    io::Error::other(format!("{}: colors.{key} geçersiz", path.display()))
                })?;
            raw.parse().map_err(|_| {
                io::Error::other(format!(
                    "{}: colors.{key} geçersiz renk: {raw}",
                    path.display()
                ))
            })
        };
        Ok(Self {
            selected: color("selected")?,
            text: color("text")?,
            muted: color("muted")?,
            separator: color("separator")?,
            search: color("search")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs};

    fn load_from(path: &std::path::Path) -> io::Result<Theme> {
        Theme::load(&Settings::load_from(path)?)
    }

    #[test]
    fn creates_defaults_and_loads_selected_theme() {
        let root = env::temp_dir().join(format!("pack-theme-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let default = load_from(&root).unwrap();
        assert_eq!(default.selected, Color::Indexed(212));

        fs::write(
            root.join("themes/new.toml"),
            DEFAULT_THEME.replace("212", "\"#123456\""),
        )
        .unwrap();
        fs::write(
            root.join("settings.toml"),
            "[appearance]\ntheme = \"new\"\n",
        )
        .unwrap();
        let custom = load_from(&root).unwrap();
        assert_eq!(custom.selected, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(
            fs::read_to_string(root.join("settings.toml")).unwrap(),
            "[appearance]\ntheme = \"new\"\n"
        );

        fs::write(
            root.join("settings.toml"),
            "[appearance]\ntheme = \"../bad\"\n",
        )
        .unwrap();
        assert!(load_from(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
