use std::{env, path::PathBuf};

/// Rutas XDG de tbard.
pub struct Paths {
    pub data: PathBuf,
    pub library: PathBuf,
    pub runtime: PathBuf,
}

impl Paths {
    pub fn new() -> anyhow::Result<Self> {
        let home = PathBuf::from(env::var("HOME")?);
        let xdg = |var: &str, fallback: &str| {
            env::var_os(var).map(PathBuf::from).unwrap_or_else(|| home.join(fallback))
        };
        let data = xdg("XDG_DATA_HOME", ".local/share").join("tbard");
        std::fs::create_dir_all(&data)?;
        Ok(Self {
            data,
            // shortcut: carpeta fija, pasa a config.toml en la Fase 6 (módulo de home-manager).
            library: home.join("Audiobooks"),
            runtime: env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(env::temp_dir),
        })
    }

    pub fn auth(&self) -> PathBuf {
        self.data.join("auth.json")
    }

    pub fn db(&self) -> PathBuf {
        self.data.join("tbard.db")
    }
}
