use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    env,
    path::{Path, PathBuf},
};

/// Rutas XDG de tbard.
#[derive(Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
    pub library: PathBuf,
    pub runtime: PathBuf,
    pub log: PathBuf,
}

/// `~/.config/tbard/config.toml` (lo genera el módulo de home-manager).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct File {
    /// Dónde se guardan los libros; por defecto `~/Audiobooks`.
    library_dir: Option<String>,
}

impl Paths {
    pub fn new() -> Result<Self> {
        let home = PathBuf::from(env::var("HOME")?);
        let xdg = |var: &str, fallback: &str| {
            env::var_os(var).map(PathBuf::from).unwrap_or_else(|| home.join(fallback))
        };
        let data = xdg("XDG_DATA_HOME", ".local/share").join("tbard");
        let cache = xdg("XDG_CACHE_HOME", ".cache").join("tbard");
        let config = xdg("XDG_CONFIG_HOME", ".config").join("tbard");
        let state = xdg("XDG_STATE_HOME", ".local/state").join("tbard");
        for dir in [&data, &cache.join("covers"), &state] {
            std::fs::create_dir_all(dir)?;
        }
        let file = load(&config.join("config.toml"))?;
        Ok(Self {
            data,
            cache,
            library: file.library_dir.map(|d| expand_home(&d, &home)).unwrap_or_else(|| home.join("Audiobooks")),
            config,
            runtime: env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(env::temp_dir),
            log: state.join("tbard.log"),
        })
    }

    pub fn auth(&self) -> PathBuf {
        self.data.join("auth.json")
    }

    pub fn db(&self) -> PathBuf {
        self.data.join("tbard.db")
    }
}

fn load(path: &Path) -> Result<File> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("{} inválido", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(e.into()),
    }
}

fn expand_home(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file() {
        let f: File = toml::from_str("library_dir = \"~/Libros\"").unwrap();
        assert_eq!(expand_home(&f.library_dir.unwrap(), Path::new("/h")), Path::new("/h/Libros"));
        assert_eq!(expand_home("/abs", Path::new("/h")), Path::new("/abs"));
        // Una clave mal escrita es un error, no se ignora en silencio.
        assert!(toml::from_str::<File>("libary_dir = \"x\"").is_err());
    }
}
