//! Where tbard keeps its files, and `config.toml`.
//!
//! Every directory is the platform's per-user location (XDG on Linux,
//! `~/Library` on macOS, `%APPDATA%`/`%LOCALAPPDATA%` on Windows). On Unix the
//! directories are created `0700` and private files `0600`, so other users on
//! the same machine cannot read credentials, the database or the logs.

use crate::i18n::{tr, Lang};
use anyhow::{bail, Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};
use std::{
    env,
    io::Write,
    path::{Path, PathBuf, MAIN_SEPARATOR},
};

#[derive(Clone)]
pub struct Paths {
    /// `config.toml` and `colors.toml`.
    pub config: PathBuf,
    /// Credentials and the database. Private.
    pub data: PathBuf,
    /// Cover images.
    pub cache: PathBuf,
    /// `tbard.log`. Private.
    pub log: PathBuf,
    /// mpv's IPC socket (Unix). Private.
    pub runtime: PathBuf,
    /// Where new downloads go (`library_dir` in config.toml).
    pub library: PathBuf,
    pub home: PathBuf,
    /// External programs; `None` when not found.
    pub mpv: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    /// Interface language; `None` until chosen on first run.
    pub language: Option<Lang>,
}

/// `config.toml`. Written by the TUI settings screen, or by home-manager
/// (`programs.tbard.settings`), which makes it read-only.
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct File {
    /// Where new downloads go; defaults to `~/Audiobooks`.
    library_dir: Option<String>,
    /// Explicit paths to the external programs, if they are not on PATH.
    mpv_path: Option<String>,
    ffmpeg_path: Option<String>,
    /// `en` or `es`.
    language: Option<String>,
}

impl Paths {
    pub fn new() -> Result<Self> {
        let base = BaseDirs::new().context("could not determine the home directory")?;
        let dirs = ProjectDirs::from("", "", "tbard").context("could not determine the home directory")?;
        let home = base.home_dir().to_owned();
        let data = dirs.data_dir().to_owned();
        let log_dir = dirs.state_dir().map(Path::to_owned).unwrap_or_else(|| dirs.data_local_dir().join("logs"));
        let runtime = dirs.runtime_dir().map(Path::to_owned).unwrap_or_else(|| data.join("run"));
        let paths = Self {
            config: dirs.config_dir().to_owned(),
            cache: dirs.cache_dir().to_owned(),
            log: log_dir.join("tbard.log"),
            runtime,
            library: home.join("Audiobooks"),
            home,
            data,
            mpv: None,
            ffmpeg: None,
            language: None,
        };
        for dir in [&paths.config, &paths.data, &paths.cache, &log_dir, &paths.runtime] {
            secure_dir(dir)?;
        }
        std::fs::create_dir_all(paths.cache.join("covers"))?;

        let file = load(&paths.config_file())?;
        let program = |name: &str, path: Option<String>| match path {
            Some(p) => Some(expand_home(&p, &paths.home)),
            None => find_in_path(name, env::var_os("PATH").as_deref()),
        };
        Ok(Self {
            library: file.library_dir.map(|d| expand_home(&d, &paths.home)).unwrap_or(paths.library.clone()),
            mpv: program("mpv", file.mpv_path),
            ffmpeg: program("ffmpeg", file.ffmpeg_path),
            language: file.language.as_deref().and_then(Lang::parse),
            ..paths
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn db(&self) -> PathBuf {
        self.data.join("tbard.db")
    }

    pub fn mpv(&self) -> Result<&Path> {
        self.mpv.as_deref().with_context(|| missing("mpv"))
    }

    pub fn ffmpeg(&self) -> Result<&Path> {
        self.ffmpeg.as_deref().with_context(|| missing("ffmpeg"))
    }

    /// Changes the download folder and saves it to config.toml. Books already
    /// downloaded stay where they are (their path is in the database).
    pub fn set_library(&mut self, input: &str) -> Result<()> {
        self.library = save_library_dir(&self.config_file(), &self.home, input)?;
        Ok(())
    }

    pub fn set_language(&mut self, lang: Lang) -> Result<()> {
        update_file(&self.config_file(), |cfg| cfg.language = Some(lang.code().to_owned()))?;
        self.language = Some(lang);
        Ok(())
    }

    /// A path for display, with `~` instead of the home directory.
    pub fn display(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rest) => format!("~{MAIN_SEPARATOR}{}", rest.display()),
            Err(_) => p.display().to_string(),
        }
    }
}

/// Error message for a missing external program, with how to install it here.
pub fn missing(program: &str) -> String {
    let hint = if cfg!(target_os = "macos") {
        "`brew install mpv ffmpeg`".to_owned()
    } else if cfg!(windows) {
        "`scoop bucket add extras; scoop install mpv ffmpeg`".to_owned()
    } else {
        format!(
            "{} `sudo apt install mpv ffmpeg`, `sudo dnf install mpv ffmpeg`, `sudo pacman -S mpv ffmpeg`",
            tr("your package manager, e.g.", "tu gestor de paquetes, p. ej.")
        )
    };
    match crate::i18n::get() {
        Lang::En => format!("{program} was not found. Install it with {hint}, or set {program}_path in config.toml"),
        Lang::Es => format!("no se encontró {program}. Instálalo con {hint}, o pon {program}_path en config.toml"),
    }
}

/// Looks for an executable named `name` in the directories of `path_var`.
fn find_in_path(name: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    env::split_paths(path_var?).map(|dir| dir.join(&file)).find(|p| p.is_file())
}

/// Creates `dir` (and its parents); on Unix makes it accessible only to the
/// current user. On Windows the per-user profile ACLs already do that.
pub fn secure_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Creates `path` if missing; on Unix makes it readable only by the current
/// user (also fixing an existing file with looser permissions).
pub fn secure_file(path: &Path) -> Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode(0o600);
        opts.open(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    opts.open(path)?;
    Ok(())
}

/// Atomically replaces `path` with `bytes`, private to the current user.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    secure_file(&tmp)?;
    let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn load(path: &Path) -> Result<File> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("invalid {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(e.into()),
    }
}

/// Applies `change` to config.toml, keeping the rest of the file.
fn update_file(file: &Path, change: impl FnOnce(&mut File)) -> Result<()> {
    if file.is_symlink() {
        bail!(
            "{}",
            tr(
                "config.toml is managed by home-manager: change it in programs.tbard.settings",
                "config.toml lo gestiona home-manager: cámbialo en programs.tbard.settings"
            )
        );
    }
    let mut cfg = load(file)?;
    change(&mut cfg);
    std::fs::create_dir_all(file.parent().context("invalid config path")?)?;
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, toml::to_string(&cfg)?)?;
    std::fs::rename(tmp, file)?;
    Ok(())
}

fn save_library_dir(file: &Path, home: &Path, input: &str) -> Result<PathBuf> {
    let input = input.trim();
    let dir = expand_home(input, home);
    if !dir.is_absolute() {
        bail!("{}", tr("use an absolute path or one starting with ~/", "usa una ruta absoluta o que empiece por ~/"));
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("{} {}", tr("could not create", "no se pudo crear"), dir.display()))?;
    update_file(file, |cfg| cfg.library_dir = Some(input.to_owned()))?;
    Ok(dir)
}

fn expand_home(p: &str, home: &Path) -> PathBuf {
    if p == "~" {
        return home.to_owned();
    }
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("tbard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn config_file() {
        let f: File = toml::from_str("library_dir = \"~/Libros\"").unwrap();
        assert_eq!(expand_home(&f.library_dir.unwrap(), Path::new("/h")), Path::new("/h/Libros"));
        assert_eq!(expand_home("/abs", Path::new("/h")), Path::new("/abs"));
        // A misspelled key is an error, not silently ignored.
        assert!(toml::from_str::<File>("libary_dir = \"x\"").is_err());
    }

    #[test]
    fn save_library_dir_roundtrip() {
        let tmp = tmp("cfg");
        let (home, file) = (tmp.join("home"), tmp.join("cfg/config.toml"));
        let dir = save_library_dir(&file, &home, " ~/Libros ").unwrap();
        assert_eq!(dir, home.join("Libros"));
        assert!(dir.is_dir());
        assert_eq!(load(&file).unwrap().library_dir.as_deref(), Some("~/Libros"));
        assert!(save_library_dir(&file, &home, "relative").is_err());
        // Saving another key keeps the existing ones.
        update_file(&file, |cfg| cfg.language = Some("es".into())).unwrap();
        let cfg = load(&file).unwrap();
        assert_eq!((cfg.library_dir.as_deref(), cfg.language.as_deref()), (Some("~/Libros"), Some("es")));

        // Managed by home-manager (a symlink into the Nix store): left alone.
        #[cfg(unix)]
        {
            std::fs::remove_file(&file).unwrap();
            std::os::unix::fs::symlink(tmp.join("store.toml"), &file).unwrap();
            assert!(save_library_dir(&file, &home, "~/Other").is_err());
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn finds_programs_on_path() {
        let tmp = tmp("path");
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let exe = if cfg!(windows) { "mpv.exe" } else { "mpv" };
        std::fs::write(b.join(exe), "").unwrap();
        let path_var = env::join_paths([&a, &b]).unwrap();
        assert_eq!(find_in_path("mpv", Some(&path_var)), Some(b.join(exe)));
        assert_eq!(find_in_path("ffmpeg", Some(&path_var)), None);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tmp("perm");
        secure_dir(&tmp).unwrap();
        let file = tmp.join("secret");
        std::fs::write(&file, "x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        secure_file(&file).unwrap();
        write_private(&tmp.join("other"), b"y").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&tmp), 0o700);
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&tmp.join("other")), 0o600);
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
