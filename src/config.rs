use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
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
    pub home: PathBuf,
}

/// `~/.config/tbard/config.toml`: lo escribe la pantalla de configuración de la
/// TUI, o home-manager (`programs.tbard.settings`), que lo deja de solo lectura.
#[derive(Serialize, Deserialize, Default)]
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
            home,
        })
    }

    pub fn auth(&self) -> PathBuf {
        self.data.join("auth.json")
    }

    pub fn db(&self) -> PathBuf {
        self.data.join("tbard.db")
    }

    /// Cambia la carpeta de descargas y la guarda en config.toml. Los libros ya
    /// descargados se quedan donde están (su ruta está en la base de datos).
    pub fn set_library(&mut self, input: &str) -> Result<()> {
        self.library = save_library_dir(&self.config.join("config.toml"), &self.home, input)?;
        Ok(())
    }

    /// Ruta para mostrar, con `~` en lugar de la carpeta personal.
    pub fn display(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => p.display().to_string(),
        }
    }
}

fn load(path: &Path) -> Result<File> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("{} inválido", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(e.into()),
    }
}

fn save_library_dir(file: &Path, home: &Path, input: &str) -> Result<PathBuf> {
    let input = input.trim();
    let dir = expand_home(input, home);
    if !dir.is_absolute() {
        bail!("usa una ruta absoluta o que empiece por ~/");
    }
    if file.is_symlink() {
        bail!("config.toml lo gestiona home-manager: cámbialo en programs.tbard.settings.library_dir");
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("no se pudo crear {}", dir.display()))?;
    let mut cfg = load(file)?;
    cfg.library_dir = Some(input.to_owned());
    std::fs::create_dir_all(file.parent().context("ruta de config inválida")?)?;
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, toml::to_string(&cfg)?)?;
    std::fs::rename(tmp, file)?;
    Ok(dir)
}

fn expand_home(p: &str, home: &Path) -> PathBuf {
    if p == "~" {
        return home.to_owned();
    }
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

    #[test]
    fn save_library_dir_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("tbard-cfg-{}", std::process::id()));
        let (home, file) = (tmp.join("home"), tmp.join("cfg/config.toml"));
        let dir = save_library_dir(&file, &home, " ~/Libros ").unwrap();
        assert_eq!(dir, home.join("Libros"));
        assert!(dir.is_dir());
        assert_eq!(load(&file).unwrap().library_dir.as_deref(), Some("~/Libros"));
        assert!(save_library_dir(&file, &home, "relativa").is_err());

        // Gestionado por home-manager: enlace al store, no se toca.
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(tmp.join("store.toml"), &file).unwrap();
        assert!(save_library_dir(&file, &home, "~/Otra").is_err());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
