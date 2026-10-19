use std::{
    collections::HashSet,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    cache::PackageCache,
    flatpak::{Catalog, FlatpakPackage, Scope},
};

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
    Flatpak,
}

impl Backend {
    pub fn command(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Yay => "yay",
            Self::Dnf => "dnf",
            Self::Flatpak => "flatpak",
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
pub enum Package {
    System(String),
    Flatpak(Box<FlatpakPackage>),
}

impl Package {
    pub fn as_str(&self) -> &str {
        match self {
            Self::System(name) => name,
            Self::Flatpak(app) => app.identifier(),
        }
    }

    pub fn search_text(&self) -> String {
        match self {
            Self::System(name) => name.clone(),
            Self::Flatpak(app) => format!(
                "{} {} {} {} {} flatpak",
                app.name,
                app.identifier(),
                app.arch(),
                app.branch(),
                app.scope.name()
            ),
        }
    }
}

impl AsRef<str> for Package {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl From<String> for Package {
    fn from(name: String) -> Self {
        Self::System(name)
    }
}
impl From<FlatpakPackage> for Package {
    fn from(app: FlatpakPackage) -> Self {
        Self::Flatpak(Box::new(app))
    }
}

impl std::fmt::Display for Package {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System(name) => formatter.write_str(name),
            Self::Flatpak(app) => write!(
                formatter,
                "{} (Flatpak, {}, {}, {})",
                app.name,
                app.remote,
                app.scope.name(),
                app.reference
            ),
        }
    }
}

pub struct PackageList {
    pub packages: Vec<Package>,
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Install(Vec<Package>),
    Remove(Vec<Package>),
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
                packages
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            Self::Remove(packages) => format!(
                "{} packages will be removed:\n{}\n\nThis may also remove configuration files and unneeded dependencies.",
                packages.len(),
                packages
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            Self::UpdateSystem => "Available package updates will be applied.".to_owned(),
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

pub struct NativePackageList {
    pub packages: Vec<String>,
    pub notice: Option<String>,
}

#[derive(Clone)]
pub struct PackageManager {
    backend: Backend,
    settings_directory: PathBuf,
    flatpak: Option<Catalog>,
}

impl PackageManager {
    pub fn new(
        choice: ManagerChoice,
        settings_directory: PathBuf,
        flatpak_enabled: bool,
    ) -> Result<Self, String> {
        let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
        let flatpak = if flatpak_enabled {
            if !command_exists("flatpak") {
                return Err("Flatpak is enabled, but flatpak was not found on PATH.".to_owned());
            }
            Some(Catalog::new()?)
        } else {
            None
        };
        let backend = match resolve_backend(choice, &os_release, command_exists) {
            Ok(backend) => backend,
            Err(_) if choice == ManagerChoice::Auto && flatpak_enabled => Backend::Flatpak,
            Err(error) => return Err(error),
        };
        Ok(Self {
            backend,
            settings_directory,
            flatpak,
        })
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn supports_aur(&self) -> bool {
        self.backend == Backend::Yay
    }

    pub fn cache_is_fresh(&self, cache: &PackageCache) -> bool {
        (self.backend == Backend::Flatpak || cache.is_fresh())
            && self.flatpak.as_ref().is_none_or(Catalog::is_fresh)
    }

    pub fn list(&self, cache: &PackageCache, source: PackageSource) -> Result<PackageList, String> {
        let native = self.native_list(cache, source);
        let mut notices = Vec::new();
        let mut packages = Vec::new();
        match native {
            Ok(list) => {
                packages.extend(list.packages.into_iter().map(Package::System));
                if let Some(notice) = list.notice {
                    notices.push(notice);
                }
            }
            Err(error) => {
                if self.flatpak.is_none()
                    || !matches!(source, PackageSource::Available | PackageSource::Installed)
                {
                    return Err(error);
                }
                notices.push(format!("System packages: {error}"));
            }
        }
        if let Some(catalog) = &self.flatpak {
            let flatpak = match source {
                PackageSource::Available => Some(catalog.available()),
                PackageSource::Installed => {
                    Some(Catalog::query(true).map(|packages| (packages, None)))
                }
                _ => None,
            };
            if let Some(result) = flatpak {
                match result {
                    Ok((apps, notice)) => {
                        packages.extend(apps.into_iter().map(Package::from));
                        if let Some(notice) = notice {
                            notices.push(notice);
                        }
                    }
                    Err(error) => notices.push(format!("Flatpak: {error}")),
                }
            }
        }
        if packages.is_empty() && !notices.is_empty() {
            return Err(notices.join("\n"));
        }
        Ok(PackageList {
            packages,
            notice: (!notices.is_empty()).then(|| notices.join("; ")),
        })
    }

    pub fn package_info(
        &self,
        package: &Package,
        installed: bool,
    ) -> Result<CommandResult, String> {
        match package {
            Package::System(package) => self.native_package_info(package, installed),
            Package::Flatpak(app) => app.info(installed),
        }
    }

    pub fn refresh_cache(&self, cache: &PackageCache) -> Result<usize, String> {
        let mut count = 0;
        let mut errors = Vec::new();
        if self.backend != Backend::Flatpak {
            match self.refresh_native_cache(cache) {
                Ok(size) => count += size,
                Err(error) => errors.push(format!("System cache: {error}")),
            }
        }
        if let Some(catalog) = &self.flatpak {
            match catalog.refresh() {
                Ok(size) => count += size,
                Err(error) => errors.push(format!("Flatpak cache: {error}")),
            }
        }
        if errors.is_empty() {
            Ok(count)
        } else {
            Err(format!(
                "Refreshed {count} packages, but some caches failed:\n{}",
                errors.join("\n")
            ))
        }
    }

    fn native_list(
        &self,
        cache: &PackageCache,
        source: PackageSource,
    ) -> Result<NativePackageList, String> {
        if self.backend == Backend::Flatpak {
            return Ok(NativePackageList {
                packages: Vec::new(),
                notice: None,
            });
        }
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

    fn native_package_info(&self, package: &str, installed: bool) -> Result<CommandResult, String> {
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

    fn refresh_native_cache(&self, cache: &PackageCache) -> Result<usize, String> {
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
        if self.backend == Backend::Flatpak {
            return Err("No native package manager is configured.".to_owned());
        }
        if matches!(action, Action::Install(packages) | Action::Remove(packages) if packages.iter().any(|package| matches!(package, Package::Flatpak(_))))
        {
            return Err(
                "Flatpak selections must be routed through the Flatpak transaction plan."
                    .to_owned(),
            );
        }
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

    fn transaction_commands(
        &self,
        action: &Action,
        root: bool,
    ) -> Result<Vec<(String, Vec<String>)>, String> {
        let mut commands = Vec::new();
        match action {
            Action::Install(packages) | Action::Remove(packages) => {
                let install = matches!(action, Action::Install(_));
                let native = packages
                    .iter()
                    .filter(|package| matches!(package, Package::System(_)))
                    .cloned()
                    .collect::<Vec<_>>();
                if !native.is_empty() {
                    commands.push(self.interactive_command(
                        &if install {
                            Action::Install(native)
                        } else {
                            Action::Remove(native)
                        },
                        root,
                    )?);
                }
                let mut groups: Vec<(Scope, String, Vec<String>)> = Vec::new();
                for package in packages {
                    let Package::Flatpak(app) = package else {
                        continue;
                    };
                    if self.flatpak.is_none() {
                        return Err("Flatpak support is disabled.".to_owned());
                    }
                    let remote = if install { app.remote.as_str() } else { "" };
                    if let Some((_, _, refs)) = groups
                        .iter_mut()
                        .find(|(scope, origin, _)| *scope == app.scope && origin == remote)
                    {
                        if !refs.contains(&app.reference) {
                            refs.push(app.reference.clone());
                        }
                    } else {
                        groups.push((app.scope, remote.to_owned(), vec![app.reference.clone()]));
                    }
                }
                for (scope, remote, refs) in groups {
                    let mut args = strings(&[
                        if install { "install" } else { "uninstall" },
                        scope.flag(),
                        "--",
                    ]);
                    if install {
                        args.push(remote);
                    }
                    args.extend(refs);
                    commands.push(("flatpak".to_owned(), args));
                }
            }
            Action::UpdateSystem => {
                if self.backend != Backend::Flatpak {
                    commands.push(self.interactive_command(action, root)?);
                }
                if self.flatpak.is_some() {
                    for scope in Scope::ALL {
                        commands.push(("flatpak".to_owned(), strings(&["update", scope.flag()])));
                    }
                }
            }
            Action::RefreshCache => {
                if self.backend != Backend::Flatpak {
                    commands.push((
                        self.backend.command().to_owned(),
                        self.query_args(PackageSource::Available),
                    ));
                }
                if self.flatpak.is_some() {
                    for scope in Scope::ALL {
                        commands.push(("flatpak".to_owned(), Catalog::query_args(scope, false)));
                    }
                }
            }
            _ => commands.push(self.interactive_command(action, root)?),
        }
        Ok(commands)
    }

    pub fn command_preview(&self, action: &Action) -> String {
        match self.transaction_commands(action, running_as_root()) {
            Ok(commands) => commands
                .into_iter()
                .map(|(program, args)| command_text(&program, &args))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(error) => error,
        }
    }

    pub fn run_interactive(&self, action: &Action) -> Result<String, String> {
        if matches!(action, Action::RefreshCache) {
            return Err("Cache refresh uses captured output.".to_owned());
        }
        let commands = self.transaction_commands(action, running_as_root())?;
        let mut completed = Vec::new();
        for (program, args) in commands {
            let command = command_text(&program, &args);
            println!("Running: {command}");
            let result = Command::new(&program).args(&args).status();
            match result {
                Ok(status) if status.success() => completed.push(command),
                result => {
                    let reason = match result {
                        Ok(status) => format!("exit code {}", exit_code(&status)),
                        Err(error) => error.to_string(),
                    };
                    return Err(format!(
                        "Failed: {command} ({reason}).\nCompleted: {}\nRemaining commands were not run.",
                        if completed.is_empty() {
                            "none".to_owned()
                        } else {
                            completed.join("; ")
                        }
                    ));
                }
            }
        }
        Ok(format!(
            "{} completed:\n{}",
            action.title(),
            completed.join("\n")
        ))
    }

    fn query_packages(&self, source: PackageSource) -> Result<NativePackageList, String> {
        let args = self.query_args(source);
        let result = run_capture(self.backend.command(), &args)?;
        let context = "Could not retrieve the package list";
        let packages = if self.backend == Backend::Dnf {
            successful_dnf_packages(result, source, context)?
        } else {
            successful_lines(result, context)?
        };
        Ok(NativePackageList {
            packages,
            notice: None,
        })
    }

    fn available_packages(&self, cache: &PackageCache) -> Result<NativePackageList, String> {
        if cache.is_fresh() {
            return cache
                .read()
                .map(|packages| NativePackageList {
                    packages,
                    notice: None,
                })
                .map_err(|error| format!("Could not read cache: {error}"));
        }

        let stale = cache.read().ok().filter(|packages| !packages.is_empty());
        match self.refresh_native_cache(cache) {
            Ok(_) => cache
                .read()
                .map(|packages| NativePackageList {
                    packages,
                    notice: None,
                })
                .map_err(|error| format!("Could not read cache: {error}")),
            Err(error) if stale.is_some() => Ok(NativePackageList {
                packages: stale.unwrap(),
                notice: Some(format!(
                    "Could not refresh cache; using the stale list: {error}"
                )),
            }),
            Err(error) => Err(error),
        }
    }

    fn orphaned_packages(&self) -> Result<NativePackageList, String> {
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

    fn recent_packages(&self) -> Result<NativePackageList, String> {
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

        Ok(NativePackageList {
            packages: parse_recent_packages(&result.stdout),
            notice: None,
        })
    }
}

fn command_text(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_owned())
        .chain(args.iter().cloned())
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

fn orphaned_package_list(result: CommandResult) -> Result<NativePackageList, String> {
    if !result.success
        && result.exit_code == Some(1)
        && result.stdout.trim().is_empty()
        && result.stderr.trim().is_empty()
    {
        return Ok(NativePackageList {
            packages: Vec::new(),
            notice: Some("No orphaned packages found.".to_owned()),
        });
    }
    let packages = successful_lines(result, "Could not retrieve orphaned packages")?;
    Ok(NativePackageList {
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

pub(crate) fn run_capture(program: &str, args: &[String]) -> Result<CommandResult, String> {
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

fn package_args(operation: &str, packages: &[Package], backend: Backend) -> Vec<String> {
    [operation.to_owned(), "--".to_owned()]
        .into_iter()
        .chain(packages.iter().map(|package| {
            if backend == Backend::Dnf {
                package
                    .as_str()
                    .rsplit_once('/')
                    .map_or(package.as_str(), |(_, name)| name)
                    .to_owned()
            } else {
                package.as_str().to_owned()
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

pub(crate) fn strings(args: &[&str]) -> Vec<String> {
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

    fn system_packages(names: &[&str]) -> Vec<Package> {
        names
            .iter()
            .map(|name| Package::System((*name).to_owned()))
            .collect()
    }

    #[test]
    fn mixed_transactions_keep_flatpak_scope_origin_architecture_and_branch() {
        let manager = PackageManager {
            backend: Backend::Pacman,
            settings_directory: PathBuf::new(),
            flatpak: Some(Catalog::new().unwrap()),
        };
        let packages = vec![
            Package::System("extra/fzf".to_owned()),
            Package::from(
                FlatpakPackage::parse(
                    Scope::User,
                    "flathub\tapp/org.example.Shared/x86_64/stable\tShared App",
                )
                .unwrap(),
            ),
            Package::from(
                FlatpakPackage::parse(
                    Scope::User,
                    "testing\tapp/org.example.Shared/aarch64/beta\tShared App",
                )
                .unwrap(),
            ),
            Package::from(
                FlatpakPackage::parse(
                    Scope::System,
                    "flathub\tapp/org.example.Shared/x86_64/stable\tShared App",
                )
                .unwrap(),
            ),
        ];
        let action = Action::Install(packages.clone());
        assert!(manager.interactive_command(&action, false).is_err());
        assert_eq!(
            manager.transaction_commands(&action, false).unwrap(),
            vec![
                (
                    "sudo".to_owned(),
                    strings(&["pacman", "-S", "--", "extra/fzf"])
                ),
                (
                    "flatpak".to_owned(),
                    strings(&[
                        "install",
                        "--user",
                        "--",
                        "flathub",
                        "app/org.example.Shared/x86_64/stable"
                    ])
                ),
                (
                    "flatpak".to_owned(),
                    strings(&[
                        "install",
                        "--user",
                        "--",
                        "testing",
                        "app/org.example.Shared/aarch64/beta"
                    ])
                ),
                (
                    "flatpak".to_owned(),
                    strings(&[
                        "install",
                        "--system",
                        "--",
                        "flathub",
                        "app/org.example.Shared/x86_64/stable"
                    ])
                ),
            ]
        );
        let remove = manager
            .transaction_commands(&Action::Remove(packages), false)
            .unwrap();
        assert_eq!(
            remove[0],
            (
                "sudo".to_owned(),
                strings(&["pacman", "-Rns", "--", "extra/fzf"])
            )
        );
        assert_eq!(
            remove[1],
            (
                "flatpak".to_owned(),
                strings(&[
                    "uninstall",
                    "--user",
                    "--",
                    "app/org.example.Shared/x86_64/stable",
                    "app/org.example.Shared/aarch64/beta"
                ])
            )
        );
        assert_eq!(
            remove[2],
            (
                "flatpak".to_owned(),
                strings(&[
                    "uninstall",
                    "--system",
                    "--",
                    "app/org.example.Shared/x86_64/stable"
                ])
            )
        );
    }

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
                flatpak: None,
            }
            .supports_aur()
        );
        assert!(
            !PackageManager {
                backend: Backend::Dnf,
                settings_directory: PathBuf::new(),
                flatpak: None,
            }
            .supports_aur()
        );
        assert!(
            PackageManager {
                backend: Backend::Yay,
                settings_directory: PathBuf::new(),
                flatpak: None,
            }
            .supports_aur()
        );
    }

    #[test]
    fn query_commands_keep_dnf_architectures_and_arch_repository_names() {
        let dnf = PackageManager {
            backend: Backend::Dnf,
            settings_directory: PathBuf::new(),
            flatpak: None,
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
                flatpak: None,
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
                flatpak: None,
            };
            let dnf = backend == Backend::Dnf;
            for (action, expected) in [
                (
                    Action::Install(system_packages(&["--unusual", "name with spaces"])),
                    strings(&[
                        if dnf { "install" } else { "-S" },
                        "--",
                        "--unusual",
                        "name with spaces",
                    ]),
                ),
                (
                    Action::Remove(system_packages(&["bash.x86_64"])),
                    strings(&[if dnf { "remove" } else { "-Rns" }, "--", "bash.x86_64"]),
                ),
                (
                    Action::Install(system_packages(&["updates/bash.x86_64"])),
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
                    Action::Remove(system_packages(&["fedora/bash.x86_64"])),
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
