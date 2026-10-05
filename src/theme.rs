use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use ratatui::style::Color;

const SETTINGS: &str = "[appearance]\ntheme = \"default\"\n";
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
    pub fn load() -> io::Result<Self> {
        let base = env::var_os("XDG_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .or_else(|| env::var_os("XDG_CONFIG_HOME"))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| {
                io::Error::other("XDG_CONFIG_DIR, XDG_CONFIG_HOME veya HOME ayarlı değil")
            })?;
        Self::load_from(&base.join("pack"))
    }

    fn load_from(directory: &Path) -> io::Result<Self> {
        let themes = directory.join("themes");
        fs::create_dir_all(&themes)?;
        write_if_missing(directory.join("settings.toml"), SETTINGS)?;
        write_if_missing(themes.join("default.toml"), DEFAULT_THEME)?;

        let settings_path = directory.join("settings.toml");
        let settings = read_toml(&settings_path)?;
        let name = settings
            .get("appearance")
            .and_then(|appearance| appearance.get("theme"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "{}: [appearance].theme gerekli",
                    settings_path.display()
                ))
            })?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(io::Error::other(
                "Tema adı yalnızca harf, rakam, '-' ve '_' içerebilir",
            ));
        }

        let path = themes.join(format!("{name}.toml"));
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

fn read_toml(path: &Path) -> io::Result<toml::Value> {
    let text = fs::read_to_string(path)?;
    text.parse()
        .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))
}

fn write_if_missing(path: PathBuf, content: &str) -> io::Result<()> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => std::io::Write::write_all(&mut file, content.as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_defaults_and_loads_selected_theme() {
        let root = env::temp_dir().join(format!("pack-theme-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let default = Theme::load_from(&root).unwrap();
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
        let custom = Theme::load_from(&root).unwrap();
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
        assert!(Theme::load_from(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
