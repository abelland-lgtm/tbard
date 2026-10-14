mod audible;
mod config;
mod db;
mod download;
mod player;
mod session;
mod sync;
mod tui;

use anyhow::{bail, Context, Result};
use audible::{client::Client, library, login};
use clap::{Parser, Subcommand};
use config::Paths;
use db::{Book, Db};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
use sync::Decision;

#[derive(Parser)]
#[command(version, about = "Audible desde la terminal (sin argumentos abre la TUI)")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Registra tbard como dispositivo en tu cuenta de Audible.
    Login {
        /// País de la tienda: us, es, uk, de, fr, ca, it, au, in, jp, br.
        #[arg(long, default_value = "us")]
        locale: String,
        /// Desarrollo: importa un auth file de `audible quickstart` sin cifrar.
        #[arg(long)]
        import: Option<PathBuf>,
    },
    /// Actualiza y muestra la biblioteca.
    Library,
    /// Descarga en máxima calidad, dividido por capítulos, sin reproducir.
    Download { query: Vec<String> },
    /// Descarga si hace falta y reproduce, sincronizando la posición con Audible.
    Play {
        /// Parte del título, autor o serie.
        query: Vec<String>,
    },
    /// Envía las posiciones pendientes.
    Sync,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::new()?;
    init_log(&paths)?;
    match cli.cmd {
        None => tui::run(paths).await,
        Some(Cmd::Login { locale, import }) => cmd_login(&paths, &locale, import).await,
        Some(Cmd::Library) => {
            let (client, db) = open(&paths)?;
            refresh(&client, &db).await?;
            for b in db.search("")? {
                let mark = if b.path.is_some() { "●" } else { " " };
                println!("{mark} {} — {} [{}]", b.title, b.authors, hms(b.runtime_ms));
            }
            Ok(())
        }
        Some(Cmd::Download { query }) => {
            let (client, db) = open(&paths)?;
            let book = find(&client, &db, &query.join(" ")).await?;
            let dir = download(&client, &db, &paths, &book).await?.0;
            println!("Listo: {}", dir.display());
            Ok(())
        }
        Some(Cmd::Play { query }) => cmd_play(&paths, &query.join(" ")).await,
        Some(Cmd::Sync) => {
            let (client, db) = open(&paths)?;
            let left = sync::flush(&client, &db).await?;
            println!("pendientes: {left}");
            Ok(())
        }
    }
}

/// Log a archivo (nunca a la terminal: la TUI la ocupa). Nivel con `TBARD_LOG`.
fn init_log(paths: &Paths) -> Result<()> {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&paths.log)?;
    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TBARD_LOG").unwrap_or_else(|_| "tbard=info".into()),
        )
        .init();
    Ok(())
}

fn open(paths: &Paths) -> Result<(Client, Db)> {
    Ok((Client::new(paths.auth())?, Db::open(&paths.db())?))
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
            println!("Abre esta URL, inicia sesión y pega aquí la URL final (empieza por …/ap/maplanding):\n\n{}\n", pending.url);
            let _ = open::that(&pending.url);
            let redirect = prompt("URL final: ")?;
            login::finish(&reqwest::Client::new(), &pending, &redirect).await?
        }
    };
    auth.save(&paths.auth())?;
    println!("Sesión guardada en {}", paths.auth().display());
    Ok(())
}

async fn cmd_play(paths: &Paths, query: &str) -> Result<()> {
    let (client, db) = open(paths)?;
    // Sin red no pasa nada: quedan en la cola.
    let _ = sync::flush(&client, &db).await;
    let book = find(&client, &db, query).await?;
    let (dir, acr) = match (&book.path, &book.acr) {
        (Some(dir), Some(acr)) => (dir.clone(), acr.clone()),
        _ => download(&client, &db, paths, &book).await?,
    };
    let files = download::load_index(&dir).await?;

    let (decision, online) = sync::start(&client, &db, &book.asin).await?;
    if !online {
        eprintln!("sin conexión con Audible; uso la posición local");
    }
    let start = match decision {
        Decision::Start(ms) => ms,
        Decision::Ask { local, remote } => {
            let ans = prompt(&format!("Posición local {} / Audible {}. ¿Cuál uso? [l/A] ", hms(local), hms(remote)))?;
            if ans.trim().eq_ignore_ascii_case("l") { local } else { remote }
        }
    };
    println!("▶ {} desde {}", book.title, hms(start));

    // mpv recibe Ctrl+C y se cierra; tbard lo ignora para poder enviar la posición final.
    tokio::spawn(async {
        while tokio::signal::ctrl_c().await.is_ok() {}
    });

    let settings = db.settings(&book.asin)?;
    let opts = player::Options { terminal: true, speed: settings.speed, volume: settings.volume };
    let socket = paths.runtime.join(format!("tbard-{}.sock", std::process::id()));
    let mut mpv = player::Mpv::spawn(&socket, &dir, &files, start, &opts).await?;
    let mut s = session::Session::new(book.asin.clone(), acr, files, start, Instant::now());
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if mpv.exited() {
            break;
        }
        // Entre capítulos time-pos no está disponible un instante.
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
    let _ = std::fs::remove_file(&socket);

    let (last, finished) = s.finish();
    db.save_position(&s.asin, last, sync::now_ms())?;
    let sent = sync::push(&client, &db, &s.asin, &s.acr, last).await;
    println!(
        "■ {} en {}{}",
        if finished { "Terminado" } else { "Pausado" },
        hms(last),
        if sent { ", sincronizado con Audible" } else { " (pendiente de enviar)" }
    );
    Ok(())
}

/// Descarga con progreso en la terminal. Devuelve (carpeta, acr).
async fn download(client: &Client, db: &Db, paths: &Paths, book: &Book) -> Result<(PathBuf, String)> {
    println!("Descargando «{}»…", book.title);
    let mut last = None;
    let d = download::book(client, &paths.library, book, |p| {
        let line = match p {
            download::Progress::Audio { done, total } => {
                format!("audio: {}% ({} / {} MB)", done * 100 / total.max(1), done >> 20, total >> 20)
            }
            download::Progress::Splitting { done, total } => format!("capítulos: {done}/{total}"),
        };
        if last.as_ref() != Some(&line) {
            eprint!("\r  {line}\x1b[K");
            last = Some(line);
        }
    })
    .await?;
    eprintln!();
    db.set_downloaded(&book.asin, &d.dir, &d.acr, &d.content_format)?;
    println!("{} en {}", d.content_format, d.dir.display());
    Ok((d.dir, d.acr))
}

fn pick(mut books: Vec<Book>) -> Result<Book> {
    match books.len() {
        0 => bail!("ningún libro coincide (prueba `tbard library` para actualizar)"),
        1 => Ok(books.remove(0)),
        n => {
            for (i, b) in books.iter().enumerate() {
                println!("{:>3}. {} — {}", i + 1, b.title, b.authors);
            }
            let i: usize = prompt(&format!("¿Cuál? [1-{n}] "))?.trim().parse().context("número inválido")?;
            if !(1..=n).contains(&i) {
                bail!("fuera de rango");
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
