mod audible;
mod config;
mod db;
mod download;
mod player;
mod sync;

use anyhow::{bail, Context, Result};
use audible::{client::Client, content, library, login, positions};
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
#[command(version, about = "Audible desde la terminal")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
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
    /// Descarga si hace falta y reproduce, sincronizando la posición con Audible.
    Play {
        /// Parte del título, autor o serie.
        query: Vec<String>,
    },
    /// Descarga en máxima calidad, dividido por capítulos, sin reproducir.
    Download {
        query: Vec<String>,
    },
    /// Envía las posiciones pendientes.
    Sync,
}

/// Cada cuánto se guarda en local y se envía a Audible durante la reproducción.
const SAVE_EVERY: Duration = Duration::from_secs(5);
const PUSH_EVERY: Duration = Duration::from_secs(120);
/// A menos de esto del final, el libro se marca como terminado.
const FINISH_MARGIN_MS: i64 = 30_000;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::new()?;
    match cli.cmd {
        Cmd::Login { locale, import } => cmd_login(&paths, &locale, import).await,
        Cmd::Library => {
            let (client, mut db) = open(&paths)?;
            refresh(&client, &mut db).await?;
            for b in db.search("")? {
                let mark = if b.path.is_some() { "●" } else { " " };
                println!("{mark} {} — {} [{}]", b.title, b.authors, hms(b.runtime_ms));
            }
            Ok(())
        }
        Cmd::Play { query } => cmd_play(&paths, &query.join(" ")).await,
        Cmd::Download { query } => {
            let (client, mut db) = open(&paths)?;
            if db.is_empty()? {
                refresh(&client, &mut db).await?;
            }
            let book = pick(db.search(&query.join(" "))?)?;
            let (dir, _) = download_book(&client, &db, &paths, &book).await?;
            println!("Listo: {}", dir.display());
            Ok(())
        }
        Cmd::Sync => {
            let (client, db) = open(&paths)?;
            let left = sync::flush(&client, &db).await?;
            println!("pendientes: {left}");
            Ok(())
        }
    }
}

fn open(paths: &Paths) -> Result<(Client, Db)> {
    Ok((Client::new(paths.auth())?, Db::open(&paths.db())?))
}

async fn refresh(client: &Client, db: &mut Db) -> Result<()> {
    db.upsert_books(&library::fetch(client).await?)
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
    let (client, mut db) = open(paths)?;
    // Sin red no pasa nada: quedan en la cola.
    let _ = sync::flush(&client, &db).await;
    if db.is_empty()? {
        refresh(&client, &mut db).await?;
    }
    let book = pick(db.search(query)?)?;

    let (dir, acr) = match (&book.path, &book.acr) {
        (Some(dir), Some(acr)) => (dir.clone(), acr.clone()),
        _ => download_book(&client, &db, paths, &book).await?,
    };
    let files = download::load_index(&dir).await?;
    let end_ms = files.last().map_or(0, |f| f.start_ms + f.length_ms);

    let remote = match positions::last(&client, &book.asin).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sin conexión con Audible ({e}); uso la posición local");
            None
        }
    };
    let start = match sync::decide(db.position(&book.asin)?, remote) {
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

    let socket = paths.runtime.join(format!("tbard-{}.sock", std::process::id()));
    let mut mpv = player::Mpv::spawn(&socket, &dir, &files, start).await?;
    let (mut pos, mut pushed) = (start, start);
    let (mut last_save, mut last_push) = (Instant::now(), Instant::now());
    let mut was_paused = false;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if mpv.exited() {
            break;
        }
        // Entre capítulos time-pos no está disponible un instante.
        let Ok(p) = mpv.position(&files).await else { continue };
        pos = p;
        let paused = mpv.paused().await.unwrap_or(false);
        if last_save.elapsed() >= SAVE_EVERY {
            db.save_position(&book.asin, pos, sync::now_ms())?;
            last_save = Instant::now();
        }
        if pos != pushed && ((paused && !was_paused) || last_push.elapsed() >= PUSH_EVERY) {
            sync::push(&client, &db, &book.asin, &acr, pos).await;
            (pushed, last_push) = (pos, Instant::now());
        }
        was_paused = paused;
    }
    let _ = std::fs::remove_file(&socket);

    let finished = end_ms > 0 && pos >= end_ms - FINISH_MARGIN_MS;
    let last = if finished { end_ms } else { pos };
    db.save_position(&book.asin, last, sync::now_ms())?;
    let sent = sync::push(&client, &db, &book.asin, &acr, last).await;
    println!(
        "■ {} en {}{}",
        if finished { "Terminado" } else { "Pausado" },
        hms(last),
        if sent { ", sincronizado con Audible" } else { " (pendiente de enviar)" }
    );
    Ok(())
}

/// Descarga en máxima calidad y divide por capítulos. Devuelve (carpeta, acr).
async fn download_book(client: &Client, db: &Db, paths: &Paths, book: &Book) -> Result<(PathBuf, String)> {
    let lic = content::license(client, &book.asin).await?;
    let dir = download::book_dir(&paths.library, &book.authors, &book.title);
    tokio::fs::create_dir_all(&dir).await?;
    println!("Descargando «{}» ({}) en {}", book.title, lic.content_format, dir.display());

    let cover = match &book.cover_url {
        Some(url) => {
            let path = dir.join("cover.jpg");
            download::fetch(client, url, &path, |_, _| {}).await.ok().map(|_| path)
        }
        None => None,
    };
    let aaxc = dir.join(format!("{}.aaxc", book.asin));
    let mut last_pct = u64::MAX;
    download::fetch(client, &lic.offline_url, &aaxc, |done, total| {
        let pct = done * 100 / total.max(1);
        if pct != last_pct {
            last_pct = pct;
            eprint!("\r  audio: {pct}% ({} / {} MB)", done >> 20, total >> 20);
        }
    })
    .await?;
    eprintln!("\n  dividiendo en {} capítulos…", lic.chapters.len());
    let tags = download::Tags { album: &book.title, artist: &book.authors };
    download::split(&aaxc, Some((&lic.key, &lic.iv)), &lic.chapters, cover.as_deref(), &tags, &dir).await?;
    // Solo se borra el AAXC cuando todos los capítulos están listos.
    tokio::fs::remove_file(&aaxc).await?;
    db.set_downloaded(&book.asin, &dir, &lic.acr, &lic.content_format)?;
    Ok((dir, lic.acr))
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

fn hms(ms: i64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}
