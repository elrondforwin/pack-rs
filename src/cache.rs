use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    time::Duration,
};

pub const CACHE_TTL: Duration = Duration::from_secs(5 * 24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct PackageCache {
    path: PathBuf,
}

impl PackageCache {
    pub fn from_environment() -> io::Result<Self> {
        let base = env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .ok_or_else(|| io::Error::other("XDG_CACHE_HOME veya HOME ayarlı değil"))?;
        Ok(Self {
            path: base.join("pack").join("cache"),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_fresh(&self) -> bool {
        fs::metadata(&self.path)
            .and_then(|metadata| metadata.modified())
            .and_then(|modified| modified.elapsed().map_err(io::Error::other))
            .is_ok_and(|age| age <= CACHE_TTL)
    }

    pub fn read(&self) -> io::Result<Vec<String>> {
        Ok(fs::read_to_string(&self.path)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    pub fn write(&self, packages: &[String]) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("geçersiz cache yolu"))?;
        fs::create_dir_all(parent)?;

        let filename = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache");
        let temporary = parent.join(format!(".{filename}.{}.tmp", process::id()));
        let mut file = fs::File::create(&temporary)?;
        for package in packages {
            writeln!(file, "{package}")?;
        }
        file.sync_all()?;
        fs::rename(temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn cache_round_trips_packages() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("pack-cache-test-{}-{unique}", process::id()));
        let cache = PackageCache {
            path: directory.join("cache"),
        };
        let packages = vec!["core/bash".to_owned(), "extra/fzf".to_owned()];

        cache.write(&packages).unwrap();

        assert!(cache.is_fresh());
        assert_eq!(cache.read().unwrap(), packages);
        fs::remove_dir_all(directory).unwrap();
    }
}
