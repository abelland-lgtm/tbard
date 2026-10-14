mod library;
mod player;
mod theme;
mod view;

use crate::{
    audible::{client::Client, library::LibraryItem},
    config::Paths,
    db::{Book, Db},
    download::{self, Downloaded, Progress},
    player::{Mpv, Options},
    session::Session,
    sync::{self, Decision},
};
use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use library::Filter;
use player::{Player, Timer};
use ratatui::widgets::ListState;
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use theme::Theme;
use tokio::sync::{mpsc, Semaphore};

/// Mensajes de las tareas en segundo plano al bucle de la UI.
enum Msg {
    Library(Result<Vec<LibraryItem>>),
    Progress(String, Progress),
    Downloaded(String, Result<Downloaded>),
    Cover(String),
    Start { asin: String, decision: Result<(Decision, bool)> },
    Pushed(bool),
}

enum Dl {
    Queued,
    Running(Progress),
}

#[derive(PartialEq)]
enum View {
    Library,
    Player,
    Settings,
}

/// Elección pendiente entre la posición local y la de Audible.
struct Ask {
    asin: String,
    local: i64,
    remote: i64,
}

pub struct App {
    paths: Paths,
    client: Arc<Client>,
    db: Db,
    tx: mpsc::UnboundedSender<Msg>,
    theme: Theme,
    /// Fecha de `colors.toml` cargada: Noctalia lo reescribe al cambiar de fondo.
    theme_mtime: Option<std::time::SystemTime>,
    books: Vec<Book>,
    visible: Vec<usize>,
    list: ListState,
    filter: Filter,
    query: String,
    searching: bool,
    /// Texto de la opción de configuración que se está editando.
    editing: Option<String>,
    downloads: HashMap<String, Dl>,
    download_slot: Arc<Semaphore>,
    /// Libro a reproducir en cuanto termine su descarga.
    play_after: Option<String>,
    picker: Picker,
    cover: Option<(String, StatefulProtocol)>,
    covers_pending: HashSet<String>,
    view: View,
    player: Option<Player>,
    ask: Option<Ask>,
    status: Option<(String, Instant)>,
    online: Option<bool>,
    quit: bool,
}

pub async fn run(paths: Paths) -> Result<()> {
    let client = Arc::new(Client::open(&paths.data)?);
    let db = Db::open(&paths.db())?;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let mut terminal = ratatui::init();
    // Kitty/Ghostty/Sixel si la terminal responde; si no, medios bloques.
    // shortcut: si la terminal no contesta (p. ej. tmux sin terminal real), el hilo de
    // consulta de ratatui-image se queda leyendo y se come la primera tecla; aceptable.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let mut app = App {
        theme: Theme::load(&paths.config.join("colors.toml")),
        theme_mtime: mtime(&paths.config.join("colors.toml")),
        paths,
        client,
        db,
        tx,
        books: Vec::new(),
        visible: Vec::new(),
        list: ListState::default().with_selected(Some(0)),
        filter: Filter::All,
        query: String::new(),
        searching: false,
        editing: None,
        downloads: HashMap::new(),
        download_slot: Arc::new(Semaphore::new(1)),
        play_after: None,
        picker,
        cover: None,
        covers_pending: HashSet::new(),
        view: View::Library,
        player: None,
        ask: None,
        status: None,
        online: None,
        quit: false,
    };
    if app.client.protection != crate::secret::Protection::Keychain {
        app.say("no system keychain: credentials are stored unencrypted (readable only by your user)");
    }
    let res = app.main_loop(&mut terminal, &mut rx).await;
    app.stop_player().await;
    ratatui::restore();
    res
}

fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl App {
    async fn main_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        rx: &mut mpsc::UnboundedReceiver<Msg>,
    ) -> Result<()> {
        // Arranque rápido: primero la caché, luego la red.
        self.reload()?;
        self.refresh_library();
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        while !self.quit {
            terminal.draw(|f| view::draw(f, self))?;
            tokio::select! {
                Some(ev) = events.next() => {
                    if let Event::Key(key) = ev? {
                        if key.kind == KeyEventKind::Press {
                            self.on_key(key).await;
                        }
                    }
                }
                Some(msg) = rx.recv() => self.on_msg(msg).await,
                _ = tick.tick() => self.on_tick().await,
            }
        }
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let selected = self.selected().map(|b| b.asin.clone());
        self.books = self.db.search("")?;
        self.refilter();
        if let Some(asin) = selected {
            if let Some(pos) = self.visible.iter().position(|&i| self.books[i].asin == asin) {
                self.list.select(Some(pos));
            }
        }
        self.ensure_cover();
        Ok(())
    }

    fn refilter(&mut self) {
        self.visible = library::visible(&self.books, self.filter, &self.query);
        let sel = self.list.selected().unwrap_or(0).min(self.visible.len().saturating_sub(1));
        self.list.select(Some(sel));
    }

    fn selected(&self) -> Option<&Book> {
        self.list.selected().and_then(|i| self.visible.get(i)).map(|&i| &self.books[i])
    }

    fn book(&self, asin: &str) -> Option<&Book> {
        self.books.iter().find(|b| b.asin == asin)
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn refresh_library(&mut self) {
        let (client, tx) = (self.client.clone(), self.tx.clone());
        let db_path = self.paths.db();
        tokio::spawn(async move {
            // Primero vaciar la cola de posiciones pendientes.
            if let Ok(db) = Db::open(&db_path) {
                let _ = sync::flush(&client, &db).await;
            }
            let _ = tx.send(Msg::Library(crate::audible::library::fetch(&client).await));
        });
    }

    /// Carga la portada del libro relevante (el seleccionado, o el que suena
    /// en la vista del reproductor), descargándola a la caché si hace falta.
    fn ensure_cover(&mut self) {
        let asin = match (&self.view, &self.player) {
            (View::Player, Some(p)) => p.session.asin.clone(),
            _ => match self.selected() {
                Some(b) => b.asin.clone(),
                None => return,
            },
        };
        if self.cover.as_ref().is_some_and(|(a, _)| *a == asin) {
            return;
        }
        let Some(book) = self.book(&asin).cloned() else { return };
        let cached = self.paths.cache.join("covers").join(format!("{asin}.jpg"));
        let path = book.path.as_ref().map(|d| d.join("cover.jpg")).filter(|p| p.exists()).unwrap_or(cached.clone());
        if let Ok(img) = image::open(&path) {
            self.cover = Some((asin, self.picker.new_resize_protocol(img)));
            return;
        }
        if let Some(url) = book.cover_url {
            if self.covers_pending.insert(asin.clone()) {
                let (client, tx) = (self.client.clone(), self.tx.clone());
                tokio::spawn(async move {
                    if download::fetch(&client, &url, &cached, |_, _| {}).await.is_ok() {
                        let _ = tx.send(Msg::Cover(asin));
                    }
                });
            }
        }
    }

    async fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Library(Ok(items)) => {
                self.online = Some(true);
                if let Err(e) = self.db.upsert_books(&items).and_then(|_| self.reload()) {
                    self.say(format!("error guardando la biblioteca: {e}"));
                }
            }
            Msg::Library(Err(e)) => {
                tracing::warn!("biblioteca: {e:#}");
                self.online = Some(false);
                self.say(format!("sin conexión con Audible: {e}"));
            }
            Msg::Progress(asin, p) => {
                self.downloads.insert(asin, Dl::Running(p));
            }
            Msg::Downloaded(asin, res) => {
                self.downloads.remove(&asin);
                match res {
                    Ok(d) => {
                        let _ = self.db.set_downloaded(&asin, &d.dir, &d.acr, &d.content_format);
                        let _ = self.reload();
                        let title = self.book(&asin).map(|b| b.title.clone()).unwrap_or_default();
                        self.say(format!("descargado: {title} ({})", d.content_format));
                        if self.play_after.as_deref() == Some(&asin) {
                            self.play_after = None;
                            self.play(asin);
                        }
                    }
                    Err(e) => {
                        tracing::error!("descarga de {asin}: {e:#}");
                        self.say(format!("error de descarga: {e:#}"));
                    }
                }
            }
            Msg::Cover(asin) => {
                self.covers_pending.remove(&asin);
                self.ensure_cover();
            }
            Msg::Start { asin, decision } => match decision {
                Ok((Decision::Start(ms), online)) => {
                    self.online = Some(online);
                    self.launch(&asin, ms).await;
                }
                Ok((Decision::Ask { local, remote }, _)) => self.ask = Some(Ask { asin, local, remote }),
                Err(e) => self.say(format!("no se pudo empezar: {e:#}")),
            },
            Msg::Pushed(ok) => {
                self.online = Some(ok);
                if let Some(p) = &mut self.player {
                    p.synced = Some(ok);
                }
            }
        }
    }

    async fn on_tick(&mut self) {
        let colors = self.paths.config.join("colors.toml");
        let m = mtime(&colors);
        if m != self.theme_mtime {
            (self.theme, self.theme_mtime) = (Theme::load(&colors), m);
        }
        let Some(p) = &mut self.player else { return };
        if p.mpv.exited() {
            self.stop_player().await;
            return;
        }
        // Entre capítulos time-pos no está disponible un instante.
        let Ok(pos) = p.mpv.position(&p.session.files).await else { return };
        let paused = p.mpv.paused().await.unwrap_or(false);
        let act = p.session.update(pos, paused, Instant::now());
        let (asin, acr) = (p.session.asin.clone(), p.session.acr.clone());
        let due = p.timer.due(Instant::now(), p.chapter());
        if due {
            p.timer = Timer::Off;
            let _ = p.mpv.set("pause", serde_json::json!(true)).await;
        }
        if act.save {
            let _ = self.db.save_position(&asin, pos, sync::now_ms());
        }
        if let Some(pos) = act.push {
            self.push_bg(asin, acr, pos);
        }
        if due {
            self.say("temporizador: pausado");
        }
    }

    /// Envía la posición sin bloquear la UI (la red puede tardar hasta 15 s).
    fn push_bg(&self, asin: String, acr: String, pos: i64) {
        let (client, tx, db_path) = (self.client.clone(), self.tx.clone(), self.paths.db());
        tokio::spawn(async move {
            let ok = match Db::open(&db_path) {
                Ok(db) => sync::push(&client, &db, &asin, &acr, pos).await,
                Err(_) => false,
            };
            let _ = tx.send(Msg::Pushed(ok));
        });
    }

    fn download(&mut self, asin: &str) {
        if self.downloads.contains_key(asin) {
            return;
        }
        let Some(book) = self.book(asin).cloned() else { return };
        if book.path.is_some() {
            return;
        }
        let ffmpeg = match self.paths.ffmpeg() {
            Ok(p) => p.to_owned(),
            Err(e) => return self.say(format!("{e:#}")),
        };
        self.downloads.insert(asin.to_owned(), Dl::Queued);
        let (client, tx, slot) = (self.client.clone(), self.tx.clone(), self.download_slot.clone());
        let library = self.paths.library.clone();
        tokio::spawn(async move {
            // Una descarga a la vez; el resto espera en cola.
            let _permit = slot.acquire().await;
            let asin = book.asin.clone();
            let res = download::book(&client, &ffmpeg, &library, &book, |p| {
                let _ = tx.send(Msg::Progress(asin.clone(), p));
            })
            .await;
            let _ = tx.send(Msg::Downloaded(asin, res));
        });
    }

    /// Reproduce: descarga primero si hace falta y decide la posición inicial.
    fn play(&mut self, asin: String) {
        let Some(book) = self.book(&asin).cloned() else { return };
        if self.player.as_ref().is_some_and(|p| p.session.asin == asin) {
            self.view = View::Player;
            return;
        }
        if book.path.is_none() {
            self.play_after = Some(asin.clone());
            self.download(&asin);
            self.say(format!("descargando «{}»; empezará al terminar", book.title));
            return;
        }
        self.say(format!("consultando la posición de «{}» en Audible…", book.title));
        let (client, tx, db_path) = (self.client.clone(), self.tx.clone(), self.paths.db());
        tokio::spawn(async move {
            let decision = match Db::open(&db_path) {
                Ok(db) => sync::start(&client, &db, &asin).await,
                Err(e) => Err(e),
            };
            let _ = tx.send(Msg::Start { asin, decision });
        });
    }

    async fn launch(&mut self, asin: &str, start: i64) {
        if let Err(e) = self.try_launch(asin, start).await {
            tracing::error!("reproducir {asin}: {e:#}");
            self.say(format!("no se pudo reproducir: {e:#}"));
        }
    }

    async fn try_launch(&mut self, asin: &str, start: i64) -> Result<()> {
        self.stop_player().await;
        let book = self.book(asin).cloned().context("libro desconocido")?;
        let dir = book.path.clone().context("libro sin descargar")?;
        let acr = book.acr.clone().context("libro sin acr")?;
        let files = download::load_index(&dir).await?;
        let settings = self.db.settings(asin)?;
        let socket = crate::player::ipc_path(&self.paths.runtime);
        let opts = Options { terminal: false, speed: settings.speed, volume: settings.volume };
        let mpv = Mpv::spawn(self.paths.mpv()?, &socket, &dir, &files, start, &opts).await?;
        self.player = Some(Player {
            mpv,
            session: Session::new(asin.to_owned(), acr, files, start, Instant::now()),
            title: book.title,
            authors: book.authors,
            socket,
            settings,
            timer: Timer::Off,
            synced: None,
        });
        self.view = View::Player;
        self.cover = None;
        self.ensure_cover();
        self.status = None;
        Ok(())
    }

    /// Cierra mpv y envía la posición final (esperando, para no perderla al salir).
    async fn stop_player(&mut self) {
        let Some(mut p) = self.player.take() else { return };
        let _ = p.mpv.quit().await;
        let _ = std::fs::remove_file(&p.socket);
        let (last, finished) = p.session.finish();
        let _ = self.db.save_position(&p.session.asin, last, sync::now_ms());
        let _ = self.db.save_settings(&p.session.asin, p.settings);
        let ok = sync::push(&self.client, &self.db, &p.session.asin, &p.session.acr, last).await;
        self.online = Some(ok);
        self.say(format!(
            "{} «{}» en {}{}",
            if finished { "terminado" } else { "detenido" },
            p.title,
            crate::hms(last),
            if ok { "" } else { " (posición pendiente de enviar)" }
        ));
        self.view = View::Library;
        let _ = self.reload();
    }

    async fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if let Some(ask) = &self.ask {
            let (asin, local, remote) = (ask.asin.clone(), ask.local, ask.remote);
            match key.code {
                KeyCode::Char('l') => {
                    self.ask = None;
                    self.launch(&asin, local).await;
                }
                KeyCode::Char('a') | KeyCode::Enter => {
                    self.ask = None;
                    self.launch(&asin, remote).await;
                }
                KeyCode::Esc => self.ask = None,
                _ => {}
            }
            return;
        }
        if let Some(buf) = &mut self.editing {
            match key.code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter => {
                    let input = std::mem::take(buf);
                    match self.paths.set_library(&input) {
                        Ok(()) => {
                            self.editing = None;
                            let dir = self.paths.display(&self.paths.library);
                            self.say(format!("las descargas nuevas irán a {dir}"));
                        }
                        Err(e) => {
                            self.editing = Some(input);
                            self.say(format!("{e:#}"));
                        }
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.query.clear();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) => self.query.push(c),
                _ => return,
            }
            self.list.select(Some(0));
            self.refilter();
            self.ensure_cover();
            return;
        }
        let res = match self.view {
            View::Library => {
                self.library_key(key);
                Ok(())
            }
            View::Player => self.player_key(key).await,
            View::Settings => {
                self.settings_key(key);
                Ok(())
            }
        };
        if let Err(e) = res {
            self.say(format!("mpv: {e}"));
        }
    }

    fn library_key(&mut self, key: KeyEvent) {
        let n = self.visible.len();
        let sel = self.list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.list.select(Some((sel + 1).min(n.saturating_sub(1)))),
            KeyCode::Up | KeyCode::Char('k') => self.list.select(Some(sel.saturating_sub(1))),
            KeyCode::PageDown => self.list.select(Some((sel + 10).min(n.saturating_sub(1)))),
            KeyCode::PageUp => self.list.select(Some(sel.saturating_sub(10))),
            KeyCode::Home | KeyCode::Char('g') => self.list.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.list.select(Some(n.saturating_sub(1))),
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Esc => {
                self.query.clear();
                self.refilter();
            }
            KeyCode::Char(c @ '1'..='5') => {
                self.filter = Filter::ALL[c as usize - '1' as usize];
                self.list.select(Some(0));
                self.refilter();
            }
            KeyCode::Char('f') => {
                self.filter = self.filter.next();
                self.list.select(Some(0));
                self.refilter();
            }
            KeyCode::Char('r') => {
                self.say("actualizando biblioteca…");
                self.refresh_library();
            }
            KeyCode::Char('d') => {
                if let Some(asin) = self.selected().map(|b| b.asin.clone()) {
                    self.download(&asin);
                }
            }
            KeyCode::Enter => {
                if let Some(asin) = self.selected().map(|b| b.asin.clone()) {
                    self.play(asin);
                }
            }
            KeyCode::Char('c') => self.view = View::Settings,
            KeyCode::Tab if self.player.is_some() => {
                self.view = View::Player;
                self.ensure_cover();
            }
            _ => return,
        }
        self.ensure_cover();
    }

    fn settings_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc | KeyCode::Tab | KeyCode::Char('c') => self.view = View::Library,
            KeyCode::Enter => self.editing = Some(self.paths.display(&self.paths.library)),
            _ => {}
        }
    }

    async fn player_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(p) = &mut self.player else {
            self.view = View::Library;
            return Ok(());
        };
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Tab | KeyCode::Esc => {
                self.view = View::Library;
                self.ensure_cover();
            }
            KeyCode::Char(' ') => p.mpv.toggle_pause().await?,
            KeyCode::Left | KeyCode::Char('h') => p.skip(-30_000).await?,
            KeyCode::Right | KeyCode::Char('l') => p.skip(30_000).await?,
            KeyCode::Char('[') => p.prev_chapter().await?,
            KeyCode::Char(']') => p.next_chapter().await?,
            KeyCode::Char('+') | KeyCode::Char('=') => p.speed(0.1).await?,
            KeyCode::Char('-') => p.speed(-0.1).await?,
            KeyCode::Char('0') => p.volume(5).await?,
            KeyCode::Char('9') => p.volume(-5).await?,
            KeyCode::Char('t') => p.timer = p.timer.next(Instant::now(), p.chapter()),
            KeyCode::Char('s') => self.stop_player().await,
            _ => {}
        }
        Ok(())
    }
}

