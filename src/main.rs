mod audible;
mod config;
mod db;
mod download;
mod i18n;
mod player;
mod secret;
mod session;
mod sync;
mod tui;

use anyhow::{bail, Context, Result};
use audible::{client::Client, library, login};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use config::Paths;
use db::{Book, Db};
use i18n::{tr, Lang};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
use sync::Decision;

#[derive(Parser)]
#[command(
    version,
    about = "Audiobook manager and listener for the terminal (no arguments opens the TUI)",
    after_help = "Inspired by Libation (https://github.com/rmcrackan/Libation) and similar projects."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Registers tbard as a device on your Audible account.
    Login {
        /// Your Audible store: us, uk, de, fr, ca, it, au, in, jp, es, br.
        #[arg(long, default_value = "us")]
        locale: String,
        /// Imports an unencrypted credentials file from `audible quickstart`.
        #[arg(long)]
        import: Option<PathBuf>,
    },
    /// Refreshes and lists your library.
    Library,
    /// Downloads a book in the best quality, split by chapter, without playing it.
    Download {
        /// Part of the title, author or series.
        query: Vec<String>,
    },
    /// Plays a book (downloading it first if needed), syncing the position with Audible.
    Play {
        /// Part of the title, author or series.
        query: Vec<String>,
    },
    /// Sends positions that could not be sent while offline.
    Sync,
    /// Removes this device from your Amazon account and deletes the saved credentials.
    Logout,
    /// Checks the installation: external programs, file locations and session.
    Doctor,
}

/// The CLI definition, with the help texts in the interface language.
fn command() -> clap::Command {
    let cmd = Cli::command();
    if i18n::get() == Lang::En {
        return cmd;
    }
    let query = |c: clap::Command| c.mut_arg("query", |a| a.help("Parte del título, autor o serie"));
    cmd.about("Gestor y reproductor de audiolibros para la terminal (sin argumentos abre la interfaz)")
        .after_help("Inspirado en Libation (https://github.com/rmcrackan/Libation) y proyectos similares.")
        .mut_subcommand("login", |c| {
            c.about("Registra tbard como dispositivo en tu cuenta de Audible")
                .mut_arg("locale", |a| a.help("Tu tienda de Audible: us, uk, de, fr, ca, it, au, in, jp, es, br"))
                .mut_arg("import", |a| a.help("Importa un archivo de credenciales de `audible quickstart` sin cifrar"))
        })
        .mut_subcommand("library", |c| c.about("Actualiza y muestra tu biblioteca"))
        .mut_subcommand("download", |c| {
            query(c.about("Descarga un libro en la mejor calidad, dividido por capítulos, sin reproducirlo"))
        })
        .mut_subcommand("play", |c| {
            query(c.about("Reproduce un libro (descargándolo si hace falta) y sincroniza la posición con Audible"))
        })
        .mut_subcommand("sync", |c| c.about("Envía las posiciones que no se pudieron enviar sin conexión"))
        .mut_subcommand("logout", |c| {
            c.about("Quita este dispositivo de tu cuenta de Amazon y borra las credenciales guardadas")
        })
        .mut_subcommand("doctor", |c| {
            c.about("Revisa la instalación: programas externos, ubicación de archivos y sesión")
        })
}

#[tokio::main]
async fn main() -> Result<()> {
    let paths = Paths::new()?;
    i18n::set(paths.language.unwrap_or_else(Lang::system));
    let cli = Cli::from_arg_matches(&command().get_matches())?;
    init_log(&paths)?;
    match cli.cmd {
        None => tui::run(paths).await,
        Some(Cmd::Login { locale, import }) => cmd_login(&paths, &locale, import).await,
        Some(Cmd::Library) => {
            let (client, db) = open(&paths)?;
            refresh(&client, &db).await?;
            let mut out = std::io::stdout().lock();
            for b in db.search("")? {
                let mark = if b.path.is_some() { "●" } else { " " };
                // Stop quietly when the reader goes away (`tbard library | head`).
                if writeln!(out, "{mark} {} — {} [{}]", b.title, b.authors, hms(b.runtime_ms)).is_err() {
                    break;
                }
            }
            Ok(())
        }
        Some(Cmd::Download { query }) => {
            let (client, db) = open(&paths)?;
            let book = find(&client, &db, &query.join(" ")).await?;
            download(&client, &db, &paths, &book).await?;
            Ok(())
        }
        Some(Cmd::Play { query }) => cmd_play(&paths, &query.join(" ")).await,
        Some(Cmd::Doctor) => cmd_doctor(&paths),
        Some(Cmd::Logout) => cmd_logout(&paths).await,
        Some(Cmd::Sync) => {
            let (client, db) = open(&paths)?;
            let left = sync::flush(&client, &db).await?;
            println!("{} {left}", tr("pending positions:", "posiciones pendientes:"));
            Ok(())
        }
    }
}

/// Logs go to a file, never to the terminal (the TUI owns it). Level via `TBARD_LOG`.
fn init_log(paths: &Paths) -> Result<()> {
    config::secure_file(&paths.log)?;
    let file = std::fs::OpenOptions::new().append(true).open(&paths.log)?;
    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TBARD_LOG").unwrap_or_else(|_| "tbard=info".into()),
        )
        .init();
    Ok(())
}

fn cmd_doctor(paths: &Paths) -> Result<()> {
    let mut problems = 0;
    let mut check = |ok: bool, label: &str, detail: String| {
        problems += usize::from(!ok);
        println!("{} {label:<9} {detail}", if ok { "✓" } else { "✗" });
    };
    println!("tbard {}\n", env!("CARGO_PKG_VERSION"));
    for (label, path) in [
        ("config", paths.config_file()),
        ("data", paths.data.clone()),
        ("cache", paths.cache.clone()),
        ("log", paths.log.clone()),
        ("library", paths.library.clone()),
    ] {
        println!("  {label:<9} {}", paths.display(&path));
    }
    println!();

    // First line of `<program> <flag>`, e.g. "mpv v0.41.0 Copyright…".
    let version = |program: &std::path::Path, flag: &str| {
        let out = std::process::Command::new(program).arg(flag).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().next().map(str::to_owned)
    };
    match paths.mpv() {
        Ok(p) => check(true, "mpv", version(p, "--version").unwrap_or_else(|| p.display().to_string())),
        Err(e) => check(false, "mpv", e.to_string()),
    }
    match paths.ffmpeg() {
        Ok(p) => {
            check(true, "ffmpeg", version(p, "-version").unwrap_or_else(|| p.display().to_string()));
            let help = std::process::Command::new(p).args(["-hide_banner", "-h", "demuxer=mov"]).output();
            let audible = help.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("audible_key"));
            let detail = if audible {
                tr("can decrypt Audible downloads", "puede descifrar las descargas de Audible")
            } else {
                tr("this ffmpeg lacks -audible_key (too old?)", "este ffmpeg no tiene -audible_key (¿muy antiguo?)")
            };
            check(audible, "", detail.into());
        }
        Err(e) => check(false, "ffmpeg", e.to_string()),
    }
    let session = tr("session", "sesión");
    match secret::load(&paths.data, &secret::OsKeychain) {
        Ok((a, protection)) => {
            check(true, session, format!("{} ({} {})", a.customer_info.name, tr("store:", "tienda:"), a.locale_code));
            let ok = protection == secret::Protection::Keychain;
            check(ok, tr("secrets", "secretos"), protection_note(protection).into());
        }
        Err(e) => check(false, session, format!("{e:#}")),
    }
    if problems > 0 {
        bail!("{problems} {}", tr("problem(s) found", "problema(s) encontrado(s)"));
    }
    Ok(())
}

/// How the credentials are protected, in words.
fn protection_note(p: secret::Protection) -> &'static str {
    match p {
        secret::Protection::Keychain => {
            tr("encrypted; the key is in the system keychain", "cifradas; la clave está en el llavero del sistema")
        }
        secret::Protection::FileOnly => tr(
            "no system keychain: stored unencrypted, readable only by your user",
            "sin llavero del sistema: guardadas sin cifrar, legibles solo por tu usuario",
        ),
    }
}

fn open(paths: &Paths) -> Result<(Client, Db)> {
    let client = Client::open(&paths.data)?;
    if client.protection != secret::Protection::Keychain {
        eprintln!("{}: {}", tr("warning", "aviso"), protection_note(client.protection));
    }
    Ok((client, Db::open(&paths.db())?))
}

async fn refresh(client: &Client, db: &Db) -> Result<()> {
    db.upsert_books(&library::fetch(client).await?)
}

async fn find(client: &Client, db: &Db, query: &str) -> Result<Book> {
    if db.is_empty()? {
        refresh(client, db).await?;
    }
    pick(db.search(query)?)
}

async fn cmd_login(paths: &Paths, locale: &str, import: Option<PathBuf>) -> Result<()> {
    let auth = match import {
        Some(path) => login::import(&path)?,
        None => {
            let pending = login::start(locale)?;
            println!(
                "{}\n\n{}\n",
                tr(
                    "Open this URL, sign in, and paste here the address you land on (it starts with …/ap/maplanding):",
                    "Abre esta URL, inicia sesión y pega aquí la dirección a la que llegas (empieza por …/ap/maplanding):"
                ),
                pending.url
            );
            let _ = open::that(&pending.url);
            let redirect = prompt(tr("Final URL: ", "URL final: "))?;
            login::finish(&reqwest::Client::new(), &pending, &redirect).await?
        }
    };
    let protection = secret::save(&paths.data, &auth, &secret::OsKeychain)?;
    println!(
        "{} {} ({})",
        tr("Logged in. Credentials saved in", "Sesión iniciada. Credenciales guardadas en"),
        paths.display(&paths.data),
        protection_note(protection)
    );
    Ok(())
}

async fn cmd_logout(paths: &Paths) -> Result<()> {
    let keys = secret::OsKeychain;
    match secret::load(&paths.data, &keys) {
        Ok((auth, _)) => match login::deregister(&reqwest::Client::new(), &auth).await {
            Ok(()) => println!(
                "{}",
                tr(
                    "This device was removed from your Amazon account.",
                    "Este dispositivo se quitó de tu cuenta de Amazon."
                )
            ),
            Err(e) => eprintln!(
                "{} ({e:#}).\n{}",
                tr(
                    "Could not remove this device from your Amazon account",
                    "No se pudo quitar este dispositivo de tu cuenta de Amazon"
                ),
                tr(
                    "Remove it under Amazon > Account > Content and Devices > Devices.",
                    "Quítalo en Amazon > Cuenta > Contenido y dispositivos > Dispositivos."
                )
            ),
        },
        Err(e) => eprintln!("{e:#}"),
    }
    secret::delete(&paths.data, &keys)?;
    println!("{}", tr("Credentials deleted from this computer.", "Credenciales borradas de este equipo."));
    Ok(())
}

async fn cmd_play(paths: &Paths, query: &str) -> Result<()> {
    let (client, db) = open(paths)?;
    // Offline is fine: whatever fails stays queued.
    let _ = sync::flush(&client, &db).await;
    let book = find(&client, &db, query).await?;
    let (dir, acr) = match (&book.path, &book.acr) {
        (Some(dir), Some(acr)) => (dir.clone(), acr.clone()),
        _ => download(&client, &db, paths, &book).await?,
    };
    let files = download::load_index(&dir).await?;

    let (decision, online) = sync::start(&client, &db, &book.asin).await?;
    if !online {
        eprintln!(
            "{}",
            tr("cannot reach Audible; using the local position", "sin conexión con Audible; uso la posición local")
        );
    }
    let start = match decision {
        Decision::Start(ms) => ms,
        Decision::Ask { local, remote } => {
            let q = match i18n::get() {
                Lang::En => format!("Local position {} / Audible {}. Which one? [l/A] ", hms(local), hms(remote)),
                Lang::Es => format!("Posición local {} / Audible {}. ¿Cuál uso? [l/A] ", hms(local), hms(remote)),
            };
            if prompt(&q)?.trim().eq_ignore_ascii_case("l") {
                local
            } else {
                remote
            }
        }
    };
    println!("▶ {} {} {}", book.title, tr("from", "desde"), hms(start));

    // Ctrl+C reaches mpv, which quits; tbard ignores it so it can still send the final position.
    tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

    let settings = db.settings(&book.asin)?;
    let opts = player::Options { terminal: true, speed: settings.speed, volume: settings.volume };
    let socket = player::ipc_path(&paths.runtime);
    let mut mpv = player::Mpv::spawn(paths.mpv()?, &socket, &dir, &files, start, &opts).await?;
    let mut s = session::Session::new(book.asin.clone(), acr, files, start, Instant::now());
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if mpv.exited() {
            break;
        }
        // Between chapters time-pos is briefly unavailable.
        let Ok(pos) = mpv.position(&s.files).await else { continue };
        let paused = mpv.paused().await.unwrap_or(false);
        let act = s.update(pos, paused, Instant::now());
        if act.save {
            db.save_position(&s.asin, pos, sync::now_ms())?;
        }
        if let Some(p) = act.push {
            sync::push(&client, &db, &s.asin, &s.acr, p).await;
        }
    }
    if cfg!(unix) {
        let _ = std::fs::remove_file(&socket);
    }

    let (last, finished) = s.finish();
    db.save_position(&s.asin, last, sync::now_ms())?;
    let sent = sync::push(&client, &db, &s.asin, &s.acr, last).await;
    println!(
        "■ {} {} {}{}",
        if finished { tr("Finished", "Terminado") } else { tr("Stopped", "Detenido") },
        tr("at", "en"),
        hms(last),
        if sent {
            tr(", synced with Audible", ", sincronizado con Audible")
        } else {
            tr(" (will be sent later)", " (se enviará después)")
        }
    );
    Ok(())
}

/// Downloads with progress on the terminal. Returns (folder, acr).
async fn download(client: &Client, db: &Db, paths: &Paths, book: &Book) -> Result<(PathBuf, String)> {
    println!("{} «{}»…", tr("Downloading", "Descargando"), book.title);
    let mut last = None;
    let d = download::book(client, paths.ffmpeg()?, &paths.library, book, |p| {
        let line = match p {
            download::Progress::Audio { done, total } => {
                format!("audio: {}% ({} / {} MB)", done * 100 / total.max(1), done >> 20, total >> 20)
            }
            download::Progress::Splitting { done, total } => {
                format!("{} {done}/{total}", tr("chapters:", "capítulos:"))
            }
        };
        if last.as_ref() != Some(&line) {
            eprint!("\r  {line}\x1b[K");
            last = Some(line);
        }
    })
    .await?;
    eprintln!();
    db.set_downloaded(&book.asin, &d.dir, &d.acr, &d.content_format)?;
    println!("{} → {}", d.content_format, paths.display(&d.dir));
    Ok((d.dir, d.acr))
}

fn pick(mut books: Vec<Book>) -> Result<Book> {
    match books.len() {
        0 => bail!(
            "{}",
            tr(
                "no book matches (try `tbard library` to refresh)",
                "ningún libro coincide (prueba `tbard library` para actualizar)"
            )
        ),
        1 => Ok(books.remove(0)),
        n => {
            for (i, b) in books.iter().enumerate() {
                println!("{:>3}. {} — {}", i + 1, b.title, b.authors);
            }
            let i: usize = prompt(&format!("{} [1-{n}] ", tr("Which one?", "¿Cuál?")))?
                .trim()
                .parse()
                .context(tr("not a number", "no es un número"))?;
            if !(1..=n).contains(&i) {
                bail!("{}", tr("out of range", "fuera de rango"));
            }
            Ok(books.swap_remove(i - 1))
        }
    }
}

fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line)
}

pub fn hms(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}
