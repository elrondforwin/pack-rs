use std::collections::HashSet;

use crate::{
    cache::PackageCache,
    package::{Backend, CommandResult, run_capture, strings},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    User,
    System,
}

impl Scope {
    pub const ALL: [Self; 2] = [Self::User, Self::System];

    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Self::User => "--user",
            Self::System => "--system",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlatpakPackage {
    pub scope: Scope,
    pub remote: String,
    pub reference: String,
    pub name: String,
    identifier: String,
}

impl FlatpakPackage {
    pub(crate) fn parse(scope: Scope, row: &str) -> Result<Self, String> {
        let fields = row.split('\t').collect::<Vec<_>>();
        let [remote, reference, name] = fields.as_slice() else {
            return Err("Invalid Flatpak row: expected origin, ref, and name columns.".to_owned());
        };
        let reference = if reference.starts_with("app/") {
            (*reference).to_owned()
        } else {
            format!("app/{reference}")
        };
        let parts = reference.split('/').collect::<Vec<_>>();
        if parts.len() != 4
            || parts[0] != "app"
            || parts[1..]
                .iter()
                .any(|part| part.is_empty() || part.chars().any(char::is_whitespace))
            || remote.is_empty()
            || remote.chars().any(char::is_whitespace)
        {
            return Err("Invalid Flatpak origin or application reference.".to_owned());
        }
        let identifier = format!("{remote}/{}", parts[1]);
        let name = if name.trim().is_empty() {
            parts[1].to_owned()
        } else {
            (*name).to_owned()
        };
        Ok(Self {
            scope,
            remote: (*remote).to_owned(),
            reference,
            name,
            identifier,
        })
    }

    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    pub fn arch(&self) -> &str {
        self.reference.split('/').nth(2).unwrap()
    }

    pub fn branch(&self) -> &str {
        self.reference.rsplit('/').next().unwrap()
    }

    fn cache_record(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.scope.name(),
            self.remote,
            self.reference,
            self.name
        )
    }

    pub fn info(&self, installed: bool) -> Result<CommandResult, String> {
        let mut args = strings(&[
            if installed { "info" } else { "remote-info" },
            self.scope.flag(),
            "--",
        ]);
        if !installed {
            args.push(self.remote.clone());
        }
        args.push(self.reference.clone());
        run_capture("flatpak", &args)
    }
}

#[derive(Clone)]
pub struct Catalog {
    cache: PackageCache,
}

impl Catalog {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            cache: PackageCache::from_environment(Backend::Flatpak)
                .map_err(|error| error.to_string())?,
        })
    }

    pub fn is_fresh(&self) -> bool {
        self.cache.is_fresh()
    }

    pub fn query_args(scope: Scope, installed: bool) -> Vec<String> {
        strings(&[
            if installed { "list" } else { "remote-ls" },
            scope.flag(),
            "--app",
            "--columns=origin:full,ref:full,name:full",
        ])
    }

    pub fn query(installed: bool) -> Result<Vec<FlatpakPackage>, String> {
        let mut packages = Vec::new();
        let mut seen = HashSet::new();
        for scope in Scope::ALL {
            let result = run_capture("flatpak", &Self::query_args(scope, installed))?;
            if !result.success {
                return Err(format!(
                    "Flatpak {} query failed:\n{}",
                    scope.name(),
                    result.display()
                ));
            }
            for row in result.stdout.lines().filter(|row| !row.trim().is_empty()) {
                let package = FlatpakPackage::parse(scope, row)?;
                if seen.insert((scope, package.remote.clone(), package.reference.clone())) {
                    packages.push(package);
                }
            }
        }
        Ok(packages)
    }

    pub fn refresh(&self) -> Result<usize, String> {
        let packages = Self::query(false)?;
        self.cache
            .write(
                &packages
                    .iter()
                    .map(FlatpakPackage::cache_record)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| format!("Could not write Flatpak cache: {error}"))?;
        Ok(packages.len())
    }

    fn read(&self) -> Result<Vec<FlatpakPackage>, String> {
        self.cache
            .read()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| {
                let (scope, row) = row
                    .split_once('\t')
                    .ok_or("Invalid Flatpak cache record.")?;
                let scope = match scope {
                    "user" => Scope::User,
                    "system" => Scope::System,
                    _ => return Err("Invalid Flatpak installation in cache.".to_owned()),
                };
                FlatpakPackage::parse(scope, row)
            })
            .collect()
    }

    pub fn available(&self) -> Result<(Vec<FlatpakPackage>, Option<String>), String> {
        if self.is_fresh() {
            return self.read().map(|packages| (packages, None));
        }
        let stale = self.read().ok();
        match self.refresh() {
            Ok(_) => self.read().map(|packages| (packages, None)),
            Err(error) => match stale {
                Some(packages) => Ok((
                    packages,
                    Some(format!("Using stale Flatpak cache: {error}")),
                )),
                None => Err(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_and_remote_refs_normalize_without_losing_scope_origin_or_branch() {
        let available = FlatpakPackage::parse(
            Scope::User,
            "flathub\tapp/org.example.App/x86_64/beta\tExample App",
        )
        .unwrap();
        let installed = FlatpakPackage::parse(
            Scope::User,
            "flathub\torg.example.App/x86_64/beta\tExample App",
        )
        .unwrap();
        assert_eq!(available, installed);
        assert_eq!(available.arch(), "x86_64");
        assert_eq!(available.branch(), "beta");
        let system = FlatpakPackage::parse(
            Scope::System,
            "flathub\torg.example.App/x86_64/beta\tExample App",
        )
        .unwrap();
        assert_ne!(installed, system);
        for row in [
            "flathub\torg.example.App\tExample",
            "\torg.example.App/x86_64/stable\tExample",
            "flathub\tapp/org.App/x86_64/stable/extra\tExample",
        ] {
            assert!(FlatpakPackage::parse(Scope::User, row).is_err());
        }
    }
}
