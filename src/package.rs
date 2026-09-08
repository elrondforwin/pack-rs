use std::{
    collections::HashSet, env, fs, os::unix::fs::PermissionsExt, path::Path, process::Command,
};

use crate::cache::PackageCache;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Helper {
    Yay,
}

impl Helper {
    fn command(self) -> &'static str {
        match self {
            Self::Yay => "yay",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageSource {
    Available,
    Installed,
    Recent,
    Aur,
    Orphaned,
}

impl PackageSource {
    pub fn title(self) -> &'static str {
        match self {
            Self::Available => "Install Packages",
            Self::Installed => "Installed Packages",
            Self::Recent => "Recently Installed Packages",
            Self::Aur => "Installed AUR Packages",
            Self::Orphaned => "Orphaned Packages",
        }
    }

    pub fn is_installed(self) -> bool {
        !matches!(self, Self::Available)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Install(Vec<String>),
    Remove(Vec<String>),
    UpdateSystem,
    ClearHelperCache,
    RefreshCache { without_aur: bool },
}

impl Action {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Install(_) => "Install Packages",
            Self::Remove(_) => "Remove Packages",
            Self::UpdateSystem => "Update System",
            Self::ClearHelperCache => "Clear Helper Cache",
            Self::RefreshCache { without_aur: false } => "Refresh Pack Cache",
            Self::RefreshCache { without_aur: true } => "Refresh Pack Cache (without AUR)",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Install(packages) => format!(
                "{} packages will be installed:\n{}",
                packages.len(),
                packages.join("\n")
            ),
            Self::Remove(packages) => format!(
                "{} packages will be removed with -Rns:\n{}\n\nThis may also remove configuration files and unneeded dependencies.",
                packages.len(),
                packages.join("\n")
            ),
            Self::UpdateSystem => "System packages will be updated.".to_owned(),
            Self::ClearHelperCache => {
                "The package manager cache will be cleared with -Scc.".to_owned()
            }
            Self::RefreshCache { without_aur: false } => {
                "The package list will be regenerated through the AUR helper.".to_owned()
            }
            Self::RefreshCache { without_aur: true } => {
                "The package list will be regenerated using pacman repositories only.".to_owned()
            }
        }
    }

    pub fn command_preview(&self, helper: Option<Helper>) -> Result<String, String> {
        match self {
            Self::RefreshCache { without_aur: true } => Ok("pacman -Sl".to_owned()),
            Self::RefreshCache { without_aur: false } => {
                Ok(format!("{} -Sl", require_helper(helper)?.command()))
            }
            Self::Install(packages) => Ok(format!(
                "{} -S {}",
                package_command(helper),
                packages.join(" ")
            )),
            Self::Remove(packages) => Ok(format!(
                "{} -Rns {}",
                package_command(helper),
                packages.join(" ")
            )),
            Self::UpdateSystem => Ok(format!("{} -Syu", package_command(helper))),
            Self::ClearHelperCache => Ok(format!("{} -Scc", package_command(helper))),
        }
    }
}

pub struct PackageList {
    pub packages: Vec<String>,
    pub notice: Option<String>,
}

#[derive(Clone)]
pub struct PackageManager {
    helper: Option<Helper>,
}

impl PackageManager {
    pub fn detect() -> Self {
        let helper = command_exists("yay").then_some(Helper::Yay);
        Self { helper }
    }

    pub fn helper(&self) -> Option<Helper> {
        self.helper
    }

    pub fn list(&self, cache: &PackageCache, source: PackageSource) -> Result<PackageList, String> {
        match source {
            PackageSource::Available => self.available_packages(cache),
            PackageSource::Installed => self.list_with_helper(&["-Qq"]),
            PackageSource::Aur => self.list_with_helper(&["-Qqm"]),
            PackageSource::Orphaned => self.orphaned_packages(),
            PackageSource::Recent => self.recent_packages(),
        }
    }

    pub fn refresh_cache(&self, cache: &PackageCache, without_aur: bool) -> Result<usize, String> {
        let program = if without_aur {
            "pacman"
        } else {
            package_command(self.helper)
        };
        let result = run_capture(program, &["-Sl".to_owned()])?;
        let packages = successful_packages(result, "Could not retrieve the package list")?;
        cache
            .write(&packages)
            .map_err(|error| format!("Could not write cache: {error}"))?;
        Ok(packages.len())
    }

    pub fn run_interactive(&self, action: &Action) -> Result<String, String> {
        let (program, args) = match action {
            Action::Install(packages) => {
                (package_command(self.helper), package_args("-S", packages))
            }
            Action::Remove(packages) => {
                (package_command(self.helper), package_args("-Rns", packages))
            }
            Action::UpdateSystem => (package_command(self.helper), vec!["-Syu".to_owned()]),
            Action::ClearHelperCache => (package_command(self.helper), vec!["-Scc".to_owned()]),
            Action::RefreshCache { .. } => {
                return Err("Cache refresh runs through captured output.".to_owned());
            }
        };
        let status = Command::new(program)
            .args(&args)
            .status()
            .map_err(|error| format!("Could not run {program}: {error}"))?;
        if status.success() {
            Ok(format!(
                "{} completed (exit code {}).",
                action.title(),
                exit_code(&status)
            ))
        } else {
            Err(format!(
                "{} failed (exit code {}).",
                action.title(),
                exit_code(&status)
            ))
        }
    }

    fn available_packages(&self, cache: &PackageCache) -> Result<PackageList, String> {
        if cache.is_fresh() {
            return cache
                .read()
                .map(|packages| PackageList {
                    packages,
                    notice: None,
                })
                .map_err(|error| format!("Could not read cache: {error}"));
        }

        let stale = cache.read().ok().filter(|packages| !packages.is_empty());
        match self.refresh_cache(cache, false) {
            Ok(_) => cache
                .read()
                .map(|packages| PackageList {
                    packages,
                    notice: None,
                })
                .map_err(|error| format!("Could not read cache: {error}")),
            Err(error) if stale.is_some() => Ok(PackageList {
                packages: stale.unwrap(),
                notice: Some(format!(
                    "Could not refresh cache; using the stale list: {error}"
                )),
            }),
            Err(error) => Err(error),
        }
    }

    fn list_with_helper(&self, args: &[&str]) -> Result<PackageList, String> {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let result = run_capture(package_command(self.helper), &args)?;
        let packages = successful_lines(result, "Could not retrieve the package list")?;
        Ok(PackageList {
            packages,
            notice: None,
        })
    }

    fn orphaned_packages(&self) -> Result<PackageList, String> {
        let helper = require_helper(self.helper)?;
        let result = run_capture(helper.command(), &["-Qtdq".to_owned()])?;
        if !result.success && result.stdout.trim().is_empty() {
            return Ok(PackageList {
                packages: Vec::new(),
                notice: Some("No orphaned packages found.".to_owned()),
            });
        }
        let packages = successful_lines(result, "Could not retrieve orphaned packages")?;
        Ok(PackageList {
            packages,
            notice: None,
        })
    }

    fn recent_packages(&self) -> Result<PackageList, String> {
        let result = run_capture(
            "expac",
            &["--timefmt=%Y-%m-%d %T".to_owned(), "%l\\t%n".to_owned()],
        )?;
        if !result.success {
            return Err(command_error(
                result,
                "Could not retrieve recently installed packages",
            ));
        }

        let mut rows = result.stdout.lines().collect::<Vec<_>>();
        rows.sort_unstable();
        let packages = rows
            .into_iter()
            .rev()
            .take(200)
            .filter_map(|row| row.split_whitespace().last())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        Ok(PackageList {
            packages: deduplicate(packages),
            notice: None,
        })
    }
}

pub struct CommandResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn run_capture(program: &str, args: &[String]) -> Result<CommandResult, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("Could not run {program}: {error}"))?;
    Ok(CommandResult {
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn successful_packages(result: CommandResult, context: &str) -> Result<Vec<String>, String> {
    if !result.success {
        return Err(command_error(result, context));
    }
    let packages = parse_sync_packages(&result.stdout);
    if packages.is_empty() {
        return Err(format!("{context}: the package list was empty."));
    }
    Ok(packages)
}

fn successful_lines(result: CommandResult, context: &str) -> Result<Vec<String>, String> {
    if !result.success {
        return Err(command_error(result, context));
    }
    Ok(deduplicate(
        result
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
    ))
}

fn parse_sync_packages(output: &str) -> Vec<String> {
    deduplicate(
        output
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                Some(format!("{}/{}", fields.next()?, fields.next()?))
            })
            .collect(),
    )
}

fn deduplicate(packages: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    packages
        .into_iter()
        .filter(|package| seen.insert(package.clone()))
        .collect()
}

fn package_args(operation: &str, packages: &[String]) -> Vec<String> {
    std::iter::once(operation.to_owned())
        .chain(packages.iter().cloned())
        .collect()
}

fn command_error(result: CommandResult, context: &str) -> String {
    let details = if result.stderr.trim().is_empty() {
        result.stdout.trim()
    } else {
        result.stderr.trim()
    };
    if details.is_empty() {
        format!(
            "{context} (exit code {}).",
            result
                .exit_code
                .map_or("bilinmiyor".to_owned(), |code| code.to_string())
        )
    } else {
        format!(
            "{context} (exit code {}):\n{details}",
            result
                .exit_code
                .map_or("bilinmiyor".to_owned(), |code| code.to_string())
        )
    }
}

fn require_helper(helper: Option<Helper>) -> Result<Helper, String> {
    helper.ok_or_else(|| "yay is required on PATH for AUR operations.".to_owned())
}

fn package_command(helper: Option<Helper>) -> &'static str {
    helper.map_or("pacman", Helper::command)
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| is_executable(&directory.join(command)))
    })
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn exit_code(status: &std::process::ExitStatus) -> String {
    status
        .code()
        .map_or("bilinmiyor".to_owned(), |code| code.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sync_package_parser_keeps_repository_and_name() {
        let output = "core bash 5.2.37-1\nextra fzf 0.66.0-1\ncore bash 5.2.37-1\n";
        assert_eq!(parse_sync_packages(output), vec!["core/bash", "extra/fzf"]);
    }

    #[test]
    fn captured_command_reads_a_fake_helper_output() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            env::temp_dir().join(format!("pack-fake-helper-{}-{unique}", std::process::id()));
        fs::write(&path, "#!/bin/sh\nprintf 'core bash 5.2.37-1\\n'\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();

        let result = run_capture(path.to_str().unwrap(), &[]).unwrap();

        assert!(result.success);
        assert_eq!(parse_sync_packages(&result.stdout), vec!["core/bash"]);
        fs::remove_file(path).unwrap();
    }
}
