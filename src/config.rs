use std::{
    collections::HashSet,
    env, fs, io,
    path::{Path, PathBuf},
};

use crate::package::ManagerChoice;

const DEFAULT_SETTINGS: &str = r#"package_manager = "auto"

[appearance]
theme = "default"
# Show package details initially; Ctrl+P toggles them while browsing.
details_visible = true

[fedora]
# Fedora/DNF only: exclude these architectures from the install-package cache.
# Empty means all architectures. Example: ["i686", "aarch64"]
# Applied only on the next automatic (5-day) or manual Pack cache refresh.
# This setting is reread on refresh; no restart is needed.
# Installed-package lists are not filtered.
architecture_blacklist = []

[flatpak]
# Optional on every distribution. Requires flatpak and configured remotes.
# Includes applications from the user and default system installations.
enabled = false
"#;

pub struct Settings {
    pub directory: PathBuf,
    pub theme: String,
    pub package_manager: ManagerChoice,
    pub flatpak_enabled: bool,
    pub details_visible: bool,
}

pub(crate) fn load_fedora_architecture_blacklist(directory: &Path) -> io::Result<HashSet<String>> {
    let path = directory.join("settings.toml");
    let data = read_toml(&path)?;
    let invalid = || {
        io::Error::other(format!(
            "{}: [fedora].architecture_blacklist must be an array of non-empty architecture names",
            path.display()
        ))
    };
    let Some(fedora) = data.get("fedora") else {
        return Ok(HashSet::new());
    };
    let fedora = fedora.as_table().ok_or_else(invalid)?;
    let Some(blacklist) = fedora.get("architecture_blacklist") else {
        return Ok(HashSet::new());
    };
    blacklist
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|value| {
            let name = value.as_str().ok_or_else(invalid)?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return Err(invalid());
            }
            Ok(name.to_owned())
        })
        .collect()
}

impl Settings {
    pub fn load() -> io::Result<Self> {
        let base = env::var_os("XDG_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .or_else(|| env::var_os("XDG_CONFIG_HOME"))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| {
                io::Error::other("XDG_CONFIG_DIR, XDG_CONFIG_HOME or HOME is required")
            })?;
        Self::load_from(&base.join("pack"))
    }

    pub(crate) fn load_from(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let path = directory.join("settings.toml");
        write_if_missing(path.clone(), DEFAULT_SETTINGS)?;
        let data = read_toml(&path)?;
        let package_manager = data
            .get("package_manager")
            .map(ManagerChoice::parse)
            .transpose()
            .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?
            .unwrap_or(ManagerChoice::Auto);
        let flatpak_enabled = match data.get("flatpak") {
            None => false,
            Some(flatpak) => {
                let table = flatpak.as_table().ok_or_else(|| {
                    io::Error::other(format!("{}: [flatpak] must be a table", path.display()))
                })?;
                match table.get("enabled") {
                    None => false,
                    Some(value) => value.as_bool().ok_or_else(|| {
                        io::Error::other(format!(
                            "{}: [flatpak].enabled must be true or false",
                            path.display()
                        ))
                    })?,
                }
            }
        };
        let details_visible = data
            .get("appearance")
            .and_then(|appearance| appearance.get("details_visible"))
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    io::Error::other(format!(
                        "{}: [appearance].details_visible must be true or false",
                        path.display()
                    ))
                })
            })
            .transpose()?
            .unwrap_or(true);
        let theme = data
            .get("appearance")
            .and_then(|appearance| appearance.get("theme"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "{}: [appearance].theme is required",
                    path.display()
                ))
            })?;
        if theme.is_empty()
            || !theme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(io::Error::other(
                "Theme names may only contain letters, numbers, '-' and '_'",
            ));
        }
        Ok(Self {
            directory: directory.to_owned(),
            theme: theme.to_owned(),
            package_manager,
            flatpak_enabled,
            details_visible,
        })
    }
}

pub(crate) fn read_toml(path: &Path) -> io::Result<toml::Value> {
    fs::read_to_string(path)?
        .parse()
        .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))
}

pub(crate) fn write_if_missing(path: PathBuf, content: &str) -> io::Result<()> {
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
    fn flatpak_is_opt_in_and_requires_a_boolean() {
        let root =
            env::temp_dir().join(format!("pack-flatpak-settings-test-{}", std::process::id()));
        assert!(!Settings::load_from(&root).unwrap().flatpak_enabled);
        let path = root.join("settings.toml");
        let appearance = "[appearance]\ntheme = \"default\"\n";
        fs::write(&path, appearance).unwrap();
        assert!(!Settings::load_from(&root).unwrap().flatpak_enabled);
        fs::write(&path, format!("{appearance}[flatpak]\nenabled = true\n")).unwrap();
        assert!(Settings::load_from(&root).unwrap().flatpak_enabled);
        for value in ["\"true\"", "42", "[]"] {
            fs::write(&path, format!("{appearance}[flatpak]\nenabled = {value}\n")).unwrap();
            assert!(Settings::load_from(&root).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fedora_blacklist_defaults_validates_and_rereads_changes() {
        let root = env::temp_dir().join(format!(
            "pack-blacklist-settings-test-{}",
            std::process::id()
        ));
        Settings::load_from(&root).unwrap();
        assert!(
            load_fedora_architecture_blacklist(&root)
                .unwrap()
                .is_empty()
        );
        let path = root.join("settings.toml");
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("Fedora/DNF only")
        );

        fs::write(&path, "[appearance]\ntheme = \"default\"\n").unwrap();
        assert!(
            load_fedora_architecture_blacklist(&root)
                .unwrap()
                .is_empty()
        );
        fs::write(&path, "[fedora]\n").unwrap();
        assert!(
            load_fedora_architecture_blacklist(&root)
                .unwrap()
                .is_empty()
        );
        fs::write(
            &path,
            "[fedora]\narchitecture_blacklist = [\"i686\", \"aarch64\", \"i686\"]\n",
        )
        .unwrap();
        assert_eq!(
            load_fedora_architecture_blacklist(&root).unwrap(),
            HashSet::from(["i686".to_owned(), "aarch64".to_owned()])
        );
        fs::write(&path, "[fedora]\narchitecture_blacklist = [\"noarch\"]\n").unwrap();
        assert_eq!(
            load_fedora_architecture_blacklist(&root).unwrap(),
            HashSet::from(["noarch".to_owned()])
        );
        for value in [
            "42",
            "\"i686\"",
            "[1]",
            "[\"\"]",
            "[\" i686\"]",
            "[\"i686*\"]",
        ] {
            fs::write(
                &path,
                format!("[fedora]\narchitecture_blacklist = {value}\n"),
            )
            .unwrap();
            assert!(
                load_fedora_architecture_blacklist(&root)
                    .unwrap_err()
                    .to_string()
                    .contains("architecture_blacklist")
            );
        }
        fs::write(&path, "fedora = 42\n").unwrap();
        assert!(load_fedora_architecture_blacklist(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn defaults_legacy_settings_and_validation() {
        let root = env::temp_dir().join(format!("pack-settings-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            Settings::load_from(&root).unwrap().package_manager,
            ManagerChoice::Auto
        );
        let path = root.join("settings.toml");
        let legacy = "[appearance]\ntheme = \"custom\"\n";
        fs::write(&path, legacy).unwrap();
        let settings = Settings::load_from(&root).unwrap();
        assert_eq!(settings.package_manager, ManagerChoice::Auto);
        assert_eq!(settings.theme, "custom");
        assert_eq!(fs::read_to_string(&path).unwrap(), legacy);
        for value in ["\"auto\"", "\"pacman\"", "\"yay\"", "\"dnf\""] {
            fs::write(&path, format!("package_manager = {value}\n{legacy}")).unwrap();
            assert!(Settings::load_from(&root).is_ok());
        }
        for value in ["\"apt\"", "42", "true", "[]"] {
            fs::write(&path, format!("package_manager = {value}\n{legacy}")).unwrap();
            assert!(
                Settings::load_from(&root)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("package_manager")
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
