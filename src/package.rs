use std::{
    collections::HashSet,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use crate::cache::PackageCache;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagerChoice {
    Auto,
    Pacman,
    Yay,
    Dnf,
}

impl ManagerChoice {
    pub fn parse(value: &toml::Value) -> Result<Self, String> {
        match value.as_str() {
            Some("auto") => Ok(Self::Auto),
            Some("pacman") => Ok(Self::Pacman),
            Some("yay") => Ok(Self::Yay),
            Some("dnf") => Ok(Self::Dnf),
            _ => Err("package_manager must be one of: auto, pacman, yay, dnf".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Pacman,
    Yay,
    Dnf,
}

impl Backend {
    pub fn command(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Yay => "yay",
            Self::Dnf => "dnf",
        }
    }
}

// Prefer the native family when both package managers happen to be installed.
fn resolve_backend(
    choice: ManagerChoice,
    os_release: &str,
    exists: impl Fn(&str) -> bool,
) -> Result<Backend, String> {
    let explicit = match choice {
        ManagerChoice::Pacman => Some(Backend::Pacman),
        ManagerChoice::Yay => Some(Backend::Yay),
        ManagerChoice::Dnf => Some(Backend::Dnf),
        ManagerChoice::Auto => None,
    };
    if let Some(backend) = explicit {
        return exists(backend.command()).then_some(backend).ok_or_else(|| {
            format!(
                "Configured package manager '{}' was not found on PATH.",
                backend.command()
            )
        });
    }
    let fedora_family = os_release.lines().any(|line| {
        line.split_once('=').is_some_and(|(key, value)| {
            matches!(key, "ID" | "ID_LIKE")
                && value
                    .trim_matches(['\"', '\''])
                    .split_whitespace()
                    .any(|id| matches!(id, "fedora" | "rhel" | "centos"))
        })
    });
    let candidates = if fedora_family {
        [Backend::Dnf, Backend::Yay, Backend::Pacman]
    } else {
        [Backend::Yay, Backend::Pacman, Backend::Dnf]
    };
    candidates
        .into_iter()
        .find(|backend| exists(backend.command()))
        .ok_or_else(|| "No supported package manager found on PATH (pacman, yay, dnf).".to_owned())
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
    ClearPackageCache,
    RefreshCache,
}

impl Action {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Install(_) => "Install Packages",
            Self::Remove(_) => "Remove Packages",
            Self::UpdateSystem => "Update System",
            Self::ClearPackageCache => "Clear Package Manager Cache",
            Self::RefreshCache => "Refresh Pack Cache",
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
                "{} packages will be removed:\n{}\n\nThis may also remove configuration files and unneeded dependencies.",
                packages.len(),
                packages.join("\n")
            ),
            Self::UpdateSystem => "System packages will be updated.".to_owned(),
            Self::ClearPackageCache => {
                "The selected package manager's cache will be cleared.".to_owned()
            }
            Self::RefreshCache => {
                "The package list will be regenerated using the selected package manager."
                    .to_owned()
            }
        }
    }
}

pub struct PackageList {
    pub packages: Vec<String>,
    pub notice: Option<String>,
}

#[derive(Clone)]
pub struct PackageManager {
    backend: Backend,
    settings_directory: PathBuf,
}

impl PackageManager {
    pub fn new(choice: ManagerChoice, settings_directory: PathBuf) -> Result<Self, String> {
        let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
        Ok(Self {
            backend: resolve_backend(choice, &os_release, command_exists)?,
            settings_directory,
        })
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn supports_aur(&self) -> bool {
        self.backend == Backend::Yay
    }

    pub fn list(&self, cache: &PackageCache, source: PackageSource) -> Result<PackageList, String> {
        match source {
            PackageSource::Available => self.available_packages(cache),
            PackageSource::Installed => self.query_packages(source),
            PackageSource::Aur if self.supports_aur() => self.query_packages(source),
            PackageSource::Aur => {
                Err("AUR operations require package_manager = \"yay\".".to_owned())
            }
            PackageSource::Orphaned => self.orphaned_packages(),
            PackageSource::Recent => self.recent_packages(),
        }
    }

    pub fn package_info(&self, package: &str, installed: bool) -> Result<CommandResult, String> {
        let args = if self.backend == Backend::Dnf {
            let (repository, package) = package
                .rsplit_once('/')
                .map_or((None, package), |(repo, name)| (Some(repo), name));
            let mut args = strings(&[
                "--quiet",
                "repoquery",
                if installed {
                    "--installed"
                } else {
                    "--available"
                },
                "--info",
                "--",
                package,
            ]);
            if !installed && let Some(repository) = repository {
                args.insert(1, format!("--repo={repository}"));
            }
            args
        } else {
            strings(&[if installed { "-Qi" } else { "-Sii" }, "--", package])
        };
        run_capture(self.backend.command(), &args)
    }

    fn query_args(&self, source: PackageSource) -> Vec<String> {
        if self.backend == Backend::Dnf {
            let filter = match source {
                PackageSource::Available => "--available",
                PackageSource::Installed | PackageSource::Recent => "--installed",
                PackageSource::Orphaned => "--unneeded",
                _ => unreachable!("source is handled without repoquery"),
            };
            let format = match source {
                PackageSource::Available => "%{repoid}\t%{name}\t%{arch}\\n",
                PackageSource::Recent => "%{installtime}\t%{from_repo}\t%{name}\t%{arch}\\n",
                _ => "%{from_repo}\t%{name}\t%{arch}\\n",
            };
            strings(&[
                "--quiet",
                "repoquery",
                filter,
                "--queryformat",
                // DNF5 does not add a record separator; DNF4's extra newline is harmless.
                format,
            ])
        } else {
            strings(&[match source {
                PackageSource::Available => "-Sl",
                PackageSource::Installed => "-Qq",
                PackageSource::Aur => "-Qqm",
                PackageSource::Orphaned => "-Qtdq",
                PackageSource::Recent => unreachable!("recent packages use expac"),
            }])
        }
    }

    pub fn refresh_cache(&self, cache: &PackageCache) -> Result<usize, String> {
        // Read this only when rebuilding the DNF cache, never when displaying a cached list.
        let blacklist = if self.backend == Backend::Dnf {
            crate::config::load_fedora_architecture_blacklist(&self.settings_directory)
                .map_err(|error| error.to_string())?
        } else {
            HashSet::new()
        };
        let result = run_capture(
            self.backend.command(),
            &self.query_args(PackageSource::Available),
        )?;
        let context = "Could not retrieve the package list";
        let mut packages = if self.backend == Backend::Dnf {
            let packages = successful_dnf_packages(result, PackageSource::Available, context)?;
            if packages.is_empty() {
                return Err(format!("{context}: the package list was empty."));
            }
            packages
        } else {
            successful_packages(result, context)?
        };
        if !blacklist.is_empty() {
            packages.retain(|package| {
                !package
                    .rsplit_once('.')
                    .is_some_and(|(_, arch)| blacklist.contains(arch))
            });
        }
        cache
            .write(&packages)
            .map_err(|error| format!("Could not write cache: {error}"))?;
        Ok(packages.len())
    }

    // Use this for both the confirmation preview and execution so they cannot drift.
    fn interactive_command(
        &self,
        action: &Action,
        root: bool,
    ) -> Result<(String, Vec<String>), String> {
        let dnf = self.backend == Backend::Dnf;
        let args = match action {
            Action::Install(packages) => {
                package_args(if dnf { "install" } else { "-S" }, packages, self.backend)
            }
            Action::Remove(packages) => {
                package_args(if dnf { "remove" } else { "-Rns" }, packages, self.backend)
            }
            Action::UpdateSystem => strings(if dnf {
                &["upgrade", "--refresh"]
            } else {
                &["-Syu"]
            }),
            Action::ClearPackageCache => strings(if dnf { &["clean", "all"] } else { &["-Scc"] }),
            Action::RefreshCache => {
                return Err("Cache refresh runs through captured output.".to_owned());
            }
        };
        let program = self.backend.command();
        if self.backend != Backend::Yay && !root {
            Ok((
                "sudo".to_owned(),
                std::iter::once(program.to_owned()).chain(args).collect(),
            ))
        } else {
            Ok((program.to_owned(), args))
        }
    }

    pub fn command_preview(&self, action: &Action) -> String {
        let (program, args) = if matches!(action, Action::RefreshCache) {
            (
                self.backend.command().to_owned(),
                self.query_args(PackageSource::Available),
            )
        } else {
            self.interactive_command(action, running_as_root())
                .expect("interactive action")
        };
        std::iter::once(program)
            .chain(args)
            .map(|arg| {
                if arg.chars().any(char::is_whitespace) {
                    format!("{arg:?}")
                } else {
                    arg
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn run_interactive(&self, action: &Action) -> Result<String, String> {
        let (program, args) = self.interactive_command(action, running_as_root())?;
        let status = Command::new(&program)
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

    fn query_packages(&self, source: PackageSource) -> Result<PackageList, String> {
        let args = self.query_args(source);
        let result = run_capture(self.backend.command(), &args)?;
        let context = "Could not retrieve the package list";
        let packages = if self.backend == Backend::Dnf {
            successful_dnf_packages(result, source, context)?
        } else {
            successful_lines(result, context)?
        };
        Ok(PackageList {
            packages,
            notice: None,
        })
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
        match self.refresh_cache(cache) {
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

    fn orphaned_packages(&self) -> Result<PackageList, String> {
        if self.backend == Backend::Dnf {
            self.query_packages(PackageSource::Orphaned)
        } else {
            let result = run_capture(
                self.backend.command(),
                &self.query_args(PackageSource::Orphaned),
            )?;
            orphaned_package_list(result)
        }
    }

    fn recent_packages(&self) -> Result<PackageList, String> {
        if self.backend == Backend::Dnf {
            return self.query_packages(PackageSource::Recent);
        }
        let result = run_capture("expac", &strings(&["--timefmt=%Y-%m-%d %T", "%l\\t%n"]))?;
        if !result.success {
            return Err(command_error(
                result,
                "Could not retrieve recently installed packages",
            ));
        }

        Ok(PackageList {
            packages: parse_recent_packages(&result.stdout),
            notice: None,
        })
    }
}

fn parse_recent_packages(output: &str) -> Vec<String> {
    let mut rows = output.lines().collect::<Vec<_>>();
    rows.sort_unstable();
    deduplicate(
        rows.into_iter()
            .rev()
            .filter_map(|row| row.split_whitespace().last())
            .map(str::to_owned)
            .collect(),
    )
    .into_iter()
    .take(200)
    .collect()
}

fn successful_dnf_packages(
    result: CommandResult,
    source: PackageSource,
    context: &str,
) -> Result<Vec<String>, String> {
    if !result.success {
        return Err(command_error(result, context));
    }
    parse_dnf_packages(&result.stdout, source)
}

fn parse_dnf_packages(output: &str, source: PackageSource) -> Result<Vec<String>, String> {
    let recent = source == PackageSource::Recent;
    let mut packages = Vec::new();
    for (index, line) in output.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let invalid = || format!("Invalid DNF package record on line {}.", index + 1);
        let fields = line.split('\t').collect::<Vec<_>>();
        let (installed_at, fields) = if recent {
            let (time, fields) = fields.split_first().ok_or_else(invalid)?;
            (time.parse::<u64>().map_err(|_| invalid())?, fields)
        } else {
            (0, fields.as_slice())
        };
        let [repository, name, arch] = fields else {
            return Err(invalid());
        };
        if name.is_empty()
            || arch.is_empty()
            || fields
                .iter()
                .any(|field| field.contains('/') || field.chars().any(char::is_whitespace))
        {
            return Err(invalid());
        }
        // DNF4 can print None for missing history; DNF5 prints an empty field.
        let repository = match *repository {
            "" | "None" | "(none)" if source.is_installed() => "@System",
            "" => return Err(invalid()),
            repository => repository,
        };
        packages.push((installed_at, format!("{repository}/{name}.{arch}")));
    }
    if recent {
        packages.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    }
    let mut packages = deduplicate(packages.into_iter().map(|(_, package)| package).collect());
    if recent {
        packages.truncate(200);
    }
    Ok(packages)
}

fn orphaned_package_list(result: CommandResult) -> Result<PackageList, String> {
    if !result.success
        && result.exit_code == Some(1)
        && result.stdout.trim().is_empty()
        && result.stderr.trim().is_empty()
    {
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

pub struct CommandResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandResult {
    pub fn display(&self) -> String {
        let mut output = String::new();
        if !self.stdout.trim().is_empty() {
            output.push_str(self.stdout.trim());
        }
        if !self.stderr.trim().is_empty() {
            if !output.is_empty() {
                output.push_str("\n\nError output:\n");
            }
            output.push_str(self.stderr.trim());
        }
        if output.is_empty() {
            output.push_str("The command produced no output.");
        }
        output
    }
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

fn package_args(operation: &str, packages: &[String], backend: Backend) -> Vec<String> {
    [operation.to_owned(), "--".to_owned()]
        .into_iter()
        .chain(packages.iter().map(|package| {
            if backend == Backend::Dnf {
                package
                    .rsplit_once('/')
                    .map_or(package.as_str(), |(_, name)| name)
                    .to_owned()
            } else {
                package.clone()
            }
        }))
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

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn running_as_root() -> bool {
    static ROOT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ROOT.get_or_init(|| {
        Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"0")
    })
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
    fn auto_selects_yay_on_arch_and_pacman_without_yay() {
        for distro in [
            "ID=arch",
            "ID=cachyos\nID_LIKE=arch",
            "ID=manjaro\nID_LIKE=\"arch\"",
        ] {
            assert_eq!(
                resolve_backend(ManagerChoice::Auto, distro, |_| true).unwrap(),
                Backend::Yay
            );
            assert_eq!(
                resolve_backend(ManagerChoice::Auto, distro, |cmd| cmd != "yay").unwrap(),
                Backend::Pacman
            );
        }
    }

    #[test]
    fn auto_prefers_dnf_on_fedora_and_derivatives() {
        for distro in ["ID=fedora", "ID=rocky\nID_LIKE=\"rhel centos fedora\""] {
            assert_eq!(
                resolve_backend(ManagerChoice::Auto, distro, |_| true).unwrap(),
                Backend::Dnf
            );
        }
        assert_eq!(
            resolve_backend(ManagerChoice::Auto, "", |cmd| cmd == "dnf").unwrap(),
            Backend::Dnf
        );
        assert!(resolve_backend(ManagerChoice::Auto, "", |_| false).is_err());
    }

    #[test]
    fn explicit_selection_never_falls_back() {
        for (choice, backend) in [
            (ManagerChoice::Pacman, Backend::Pacman),
            (ManagerChoice::Yay, Backend::Yay),
            (ManagerChoice::Dnf, Backend::Dnf),
        ] {
            assert_eq!(
                resolve_backend(choice, "ID=fedora", |_| true).unwrap(),
                backend
            );
            assert!(resolve_backend(choice, "ID=arch", |cmd| cmd != backend.command()).is_err());
        }
        assert!(
            !PackageManager {
                backend: Backend::Pacman,
                settings_directory: PathBuf::new(),
            }
            .supports_aur()
        );
        assert!(
            !PackageManager {
                backend: Backend::Dnf,
                settings_directory: PathBuf::new(),
            }
            .supports_aur()
        );
        assert!(
            PackageManager {
                backend: Backend::Yay,
                settings_directory: PathBuf::new(),
            }
            .supports_aur()
        );
    }

    #[test]
    fn query_commands_keep_dnf_architectures_and_arch_repository_names() {
        let dnf = PackageManager {
            backend: Backend::Dnf,
            settings_directory: PathBuf::new(),
        };
        for (source, flag, format) in [
            (
                PackageSource::Available,
                "--available",
                "%{repoid}\t%{name}\t%{arch}\\n",
            ),
            (
                PackageSource::Installed,
                "--installed",
                "%{from_repo}\t%{name}\t%{arch}\\n",
            ),
            (
                PackageSource::Orphaned,
                "--unneeded",
                "%{from_repo}\t%{name}\t%{arch}\\n",
            ),
            (
                PackageSource::Recent,
                "--installed",
                "%{installtime}\t%{from_repo}\t%{name}\t%{arch}\\n",
            ),
        ] {
            assert_eq!(
                dnf.query_args(source),
                strings(&["--quiet", "repoquery", flag, "--queryformat", format])
            );
        }
        for backend in [Backend::Pacman, Backend::Yay] {
            let manager = PackageManager {
                backend,
                settings_directory: PathBuf::new(),
            };
            assert_eq!(
                manager.query_args(PackageSource::Available),
                strings(&["-Sl"])
            );
            assert_eq!(
                manager.query_args(PackageSource::Orphaned),
                strings(&["-Qtdq"])
            );
        }
    }

    #[test]
    fn actions_use_backend_commands_and_only_elevate_native_managers() {
        for backend in [Backend::Pacman, Backend::Yay, Backend::Dnf] {
            let manager = PackageManager {
                backend,
                settings_directory: PathBuf::new(),
            };
            let dnf = backend == Backend::Dnf;
            for (action, expected) in [
                (
                    Action::Install(strings(&["--unusual", "name with spaces"])),
                    strings(&[
                        if dnf { "install" } else { "-S" },
                        "--",
                        "--unusual",
                        "name with spaces",
                    ]),
                ),
                (
                    Action::Remove(strings(&["bash.x86_64"])),
                    strings(&[if dnf { "remove" } else { "-Rns" }, "--", "bash.x86_64"]),
                ),
                (
                    Action::Install(strings(&["updates/bash.x86_64"])),
                    strings(&[
                        if dnf { "install" } else { "-S" },
                        "--",
                        if dnf {
                            "bash.x86_64"
                        } else {
                            "updates/bash.x86_64"
                        },
                    ]),
                ),
                (
                    Action::Remove(strings(&["fedora/bash.x86_64"])),
                    strings(&[
                        if dnf { "remove" } else { "-Rns" },
                        "--",
                        if dnf {
                            "bash.x86_64"
                        } else {
                            "fedora/bash.x86_64"
                        },
                    ]),
                ),
                (
                    Action::UpdateSystem,
                    strings(if dnf {
                        &["upgrade", "--refresh"]
                    } else {
                        &["-Syu"]
                    }),
                ),
                (
                    Action::ClearPackageCache,
                    strings(if dnf { &["clean", "all"] } else { &["-Scc"] }),
                ),
            ] {
                assert_eq!(
                    manager.interactive_command(&action, true).unwrap(),
                    (backend.command().to_owned(), expected.clone())
                );
                let (program, args) = manager.interactive_command(&action, false).unwrap();
                if backend == Backend::Yay {
                    assert_eq!((program.as_str(), args), ("yay", expected));
                } else {
                    assert_eq!(program, "sudo");
                    assert_eq!(
                        args,
                        std::iter::once(backend.command().to_owned())
                            .chain(expected)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }

    #[test]
    fn dnf_records_preserve_repositories_architectures_and_unknown_installed_origins() {
        assert_eq!(
            parse_dnf_packages(
                "fedora\tbash\tx86_64\n\nupdates\tbash\tx86_64\nfedora\tbash\ti686\nfedora\tbash\tx86_64\n",
                PackageSource::Available,
            ).unwrap(),
            strings(&["fedora/bash.x86_64", "updates/bash.x86_64", "fedora/bash.i686"])
        );
        for repository in ["", "None", "(none)", "@System"] {
            assert_eq!(
                parse_dnf_packages(
                    &format!("{repository}\tlocal-app\tx86_64\n"),
                    PackageSource::Installed
                )
                .unwrap(),
                strings(&["@System/local-app.x86_64"])
            );
        }
        for invalid in [
            "bash.x86_64",
            "fedora\tbash\n",
            "fedora\tbash\t\n",
            "fedora\tbash\tx86_64\textra\n",
        ] {
            assert!(parse_dnf_packages(invalid, PackageSource::Available).is_err());
        }
    }

    #[test]
    fn recent_packages_sort_by_install_time_and_deduplicate() {
        assert_eq!(
            parse_dnf_packages(
                "9\tfedora\tbash\tx86_64\n100\tupdates\tfzf\tnoarch\n99\tfedora\tbash\tx86_64\n",
                PackageSource::Recent
            )
            .unwrap(),
            strings(&["updates/fzf.noarch", "fedora/bash.x86_64"])
        );
        assert_eq!(
            parse_recent_packages("2026-01-01 01:00:00 bash\n2026-10-18 00:00:00 fzf\n"),
            strings(&["fzf", "bash"])
        );
        let rows = (0..250)
            .map(|i| format!("{i}\tupdates\tpkg{i}\tnoarch\n"))
            .collect::<String>();
        let recent = parse_dnf_packages(&rows, PackageSource::Recent).unwrap();
        assert_eq!(recent.len(), 200);
        assert_eq!(recent.first().unwrap(), "updates/pkg249.noarch");
        assert_eq!(recent.last().unwrap(), "updates/pkg50.noarch");
    }

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

    #[test]
    fn orphan_query_reports_errors_and_accepts_empty_result() {
        let empty = CommandResult {
            success: false,
            exit_code: Some(1),
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(orphaned_package_list(empty).unwrap().packages.is_empty());

        let failed = CommandResult {
            success: false,
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "database unavailable".to_owned(),
        };
        assert!(
            matches!(orphaned_package_list(failed), Err(error) if error.contains("database unavailable"))
        );
    }
}
