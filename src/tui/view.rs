//! Drawing. A retro terminal look: one accent color (Noctalia's `primary`),
//! big block-letter headings, bracketed labels, dot leaders and
//! `├─ KEY value` rows.

use super::{library::Filter, player::Timer, theme::Theme, App, Dl, View};
use crate::{
    db::Book,
    download::Progress,
    hms,
    i18n::{self, tr, Lang},
    player::locate,
};
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use ratatui_image::{protocol::StatefulProtocol, StatefulImage};
use std::time::{Duration, Instant};
use tui_big_text::{BigText, PixelSize};

const SIDEBAR_WIDTH: u16 = 25;

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = app.theme.clone();
    f.render_widget(Block::new().style(Style::new().bg(t.surface).fg(t.on_surface)), f.area());

    let [body, footer_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    let [side, main] = Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)]).areas(body);
    sidebar(f, app, side);
    let main = main.inner(Margin::new(2, 0));
    match app.view {
        View::Library => library(f, app, main),
        View::Player => player(f, app, main),
        View::Settings => settings(f, app, main),
        View::Language => language(f, app, main),
    }
    footer(f, app, footer_area);

    if let Some(ask) = &app.ask {
        let area = centered(f.area(), 50, 8);
        f.render_widget(Clear, area);
        let inv = Style::new().bg(t.primary).fg(t.on_primary);
        let lines = vec![
            Line::from(format!(" {}", tr("THE POSITIONS DO NOT MATCH", "LAS POSICIONES NO COINCIDEN"))).bold(),
            Line::from(""),
            Line::from(format!(" ├─ {:<10}{}", tr("HERE", "AQUÍ"), hms(ask.local))),
            Line::from(format!(" └─ {:<10}{}", "AUDIBLE", hms(ask.remote))),
            Line::from(""),
            Line::from(format!(
                " {}",
                tr("[L] here   [A] Audible   [ESC] cancel", "[L] aquí   [A] Audible   [ESC] cancelar")
            ))
            .bold(),
        ];
        let block = Block::bordered().border_type(BorderType::Thick).border_style(inv);
        f.render_widget(Paragraph::new(lines).block(block).style(inv), area);
    }
}

// ── Building blocks ───────────────────────────────────────────────────────

/// `[K] label`: the key in the accent color, brackets and label dimmed.
fn key<'a>(t: &Theme, k: &'a str, label: &'a str) -> Vec<Span<'a>> {
    vec![
        Span::styled("[", Style::new().fg(t.outline)),
        Span::styled(k, Style::new().fg(t.primary).bold()),
        Span::styled("] ", Style::new().fg(t.outline)),
        Span::styled(label, Style::new().fg(t.on_surface_variant)),
        Span::raw("  "),
    ]
}

/// `[ label: value ]`, right-aligned.
fn stat(t: &Theme, label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled("[ ", Style::new().fg(t.outline)),
        Span::styled(format!("{label}: "), Style::new().fg(t.on_surface_variant)),
        Span::styled(value, Style::new().fg(t.primary).bold()),
        Span::styled(" ]", Style::new().fg(t.outline)),
    ])
    .right_aligned()
}

/// `[■■■■····]`
fn bar(ratio: f64, cells: usize) -> String {
    let filled = ((ratio.clamp(0.0, 1.0) * cells as f64).round() as usize).min(cells);
    format!("[{}{}]", "■".repeat(filled), "·".repeat(cells - filled))
}

/// Cuts `s` to `max` characters, ending in `…`.
// shortcut: counts chars, not cell width; only wrong for CJK titles.
fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `left ........ right`, filling `width` columns.
fn leader<'a>(mut left: Vec<Span<'a>>, right: Vec<Span<'a>>, width: usize, dots: Style) -> Line<'a> {
    let used: usize = left.iter().chain(&right).map(Span::width).sum();
    let gap = width.saturating_sub(used);
    if gap >= 3 {
        left.push(Span::styled(format!(" {} ", ".".repeat(gap - 2)), dots));
    } else {
        left.push(Span::raw(" ".repeat(gap)));
    }
    left.extend(right);
    Line::from(left)
}

/// A `├─ KEY      value` row.
fn kv<'a>(t: &Theme, last: bool, k: &str, v: Vec<Span<'a>>, on_card: bool) -> Line<'a> {
    let (dim, strong) = if on_card {
        (Style::new().fg(t.on_primary).add_modifier(Modifier::DIM), Style::new().fg(t.on_primary).bold())
    } else {
        (Style::new().fg(t.outline), Style::new().fg(t.primary).bold())
    };
    let mut spans = vec![Span::styled(if last { "└─ " } else { "├─ " }, dim), Span::styled(format!("{k:<10}"), strong)];
    spans.extend(v);
    Line::from(spans)
}

fn heading(t: &Theme, text: &str) -> BigText<'static> {
    BigText::builder()
        .pixel_size(PixelSize::Quadrant)
        .style(Style::new().fg(t.primary))
        .lines(vec![Line::from(text.to_uppercase())])
        .build()
}

fn separator(t: &Theme, width: u16) -> Line<'static> {
    Line::styled("┄".repeat(width as usize), Style::new().fg(t.outline))
}

/// Header: big title on the left, figures on the right, a dashed line below.
pub(super) fn header(f: &mut Frame, t: &Theme, area: Rect, title: &str, stats: Vec<Line<'static>>) -> Rect {
    let [head, sep, rest] =
        Layout::vertical([Constraint::Length(4), Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let [big, right] = Layout::horizontal([Constraint::Min(0), Constraint::Length(32)]).areas(head);
    f.render_widget(heading(t, title), big);
    f.render_widget(Paragraph::new(stats), right);
    f.render_widget(separator(t, sep.width), sep);
    rest
}

/// Actions with their key boxed on the right, Minitel style.
pub(super) fn actions<'a>(t: &Theme, items: &[(&'a str, &'a str)]) -> Vec<Line<'a>> {
    items
        .iter()
        .map(|&(label, k)| {
            Line::from(vec![
                Span::styled(format!("{label}  "), Style::new().fg(t.on_surface)),
                Span::styled(format!(" {k:^6} "), Style::new().bg(t.primary).fg(t.on_primary).bold()),
            ])
            .right_aligned()
        })
        .collect()
}

// ── Sidebar and footer ────────────────────────────────────────────────────

fn sidebar(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = Block::new().borders(Borders::RIGHT).border_style(Style::new().fg(t.outline));
    let inner = block.inner(area).inner(Margin::new(1, 1));
    f.render_widget(block, area);
    let w = inner.width as usize;

    let item = |active: bool, label: &str, count: Option<String>| {
        let style = if active { Style::new().fg(t.primary).bold() } else { Style::new().fg(t.on_surface) };
        let left = vec![Span::styled(if active { "▸ " } else { "  " }, style), Span::styled(label.to_owned(), style)];
        let right = count.map(|c| vec![Span::styled(c, Style::new().fg(t.on_surface_variant))]).unwrap_or_default();
        let used: usize = left.iter().chain(&right).map(Span::width).sum();
        let mut spans = left;
        spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
        spans.extend(right);
        Line::from(spans)
    };

    let mut lines = vec![
        Line::styled("░T░B░A░R░D░", Style::new().fg(t.primary).bold()),
        Line::styled(tr("audiobooks · tty", "audiolibros · tty"), Style::new().fg(t.on_surface_variant)),
        Line::from(""),
    ];
    for (i, filter) in Filter::ALL.into_iter().enumerate() {
        let active = app.view == View::Library && app.filter == filter;
        lines.push(item(active, &format!("{} {}", i + 1, filter.label()), Some(filter.count(&app.books).to_string())));
    }
    lines.push(Line::styled("┄".repeat(w), Style::new().fg(t.outline)));
    let playing = app.player.as_ref().map(|p| if p.session.paused { "⏸" } else { "▶" }.to_owned());
    lines.push(item(app.view == View::Player, tr("Player", "Reproductor"), playing));
    if let Some(p) = &app.player {
        let dim = Style::new().fg(t.on_surface_variant);
        lines.push(Line::styled(format!("  {}", trunc(&p.title, w.saturating_sub(2))), dim));
        lines.push(Line::styled(format!("  {}", hms(p.session.pos)), dim));
    }
    let in_settings = matches!(app.view, View::Settings | View::Language);
    lines.push(item(in_settings, tr("Settings", "Configuración"), None));
    f.render_widget(Paragraph::new(lines), inner);

    let [_, bottom] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let dot = match app.online {
        Some(true) => Span::styled("● ", Style::new().fg(t.primary)),
        Some(false) => Span::styled("● ", Style::new().fg(t.error)),
        None => Span::styled("○ ", Style::new().fg(t.on_surface_variant)),
    };
    let version = Span::styled(concat!("tbard v", env!("CARGO_PKG_VERSION")), Style::new().fg(t.on_surface_variant));
    f.render_widget(Line::from(vec![dot, version]), bottom);
}

fn footer(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let keys: Vec<(&str, &str)> = match (&app.view, app.searching, app.editing.is_some()) {
        (_, _, true) => vec![("⏎", tr("Save", "Guardar")), ("ESC", tr("Cancel", "Cancelar"))],
        (_, true, _) => vec![("⏎", tr("Accept", "Aceptar")), ("ESC", tr("Clear", "Borrar"))],
        (View::Library, ..) => vec![
            ("/", tr("Search", "Buscar")),
            ("↑↓", tr("Move", "Navegar")),
            ("1-5", tr("Section", "Sección")),
            ("⏎", tr("Listen", "Escuchar")),
            ("D", tr("Download", "Descargar")),
            ("R", tr("Refresh", "Actualizar")),
            ("C", tr("Settings", "Config")),
            ("TAB", tr("Player", "Reproductor")),
            ("Q", tr("Quit", "Salir")),
        ],
        (View::Player, ..) => vec![
            ("␣", tr("Pause", "Pausa")),
            ("←→", "30 s"),
            ("[ ]", tr("Chapter", "Capítulo")),
            ("+-", tr("Speed", "Velocidad")),
            ("9 0", tr("Volume", "Volumen")),
            ("T", tr("Sleep timer", "Temporizador")),
            ("S", tr("Stop", "Parar")),
            ("TAB", tr("Library", "Biblioteca")),
            ("Q", tr("Quit", "Salir")),
        ],
        (View::Settings, ..) => vec![
            ("↑↓", tr("Move", "Navegar")),
            ("⏎", tr("Change", "Cambiar")),
            ("ESC", tr("Back", "Volver")),
            ("Q", tr("Quit", "Salir")),
        ],
        (View::Language, ..) => vec![("↑↓", tr("Move", "Navegar")), ("⏎", tr("Choose", "Elegir"))],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.extend(key(t, k, label));
    }
    f.render_widget(Line::from(spans), area);

    // On the right: the latest notice (for 6 s) or the connection state.
    let right = match &app.status {
        Some((m, at)) if at.elapsed() < Duration::from_secs(6) => {
            Span::styled(format!(" {} ", trunc(m, area.width as usize / 2)), Style::new().fg(t.tertiary))
        }
        _ => match app.online {
            Some(true) => Span::styled(format!(" ● {} ", tr("online", "conectado")), Style::new().fg(t.primary)),
            Some(false) => Span::styled(format!(" ● {} ", tr("offline", "sin conexión")), Style::new().fg(t.error)),
            None => {
                Span::styled(format!(" ○ {} ", tr("connecting", "conectando")), Style::new().fg(t.on_surface_variant))
            }
        },
    };
    let w = (right.width() as u16).min(area.width);
    let r = Rect { x: area.right() - w, width: w, ..area };
    f.render_widget(Clear, r);
    f.render_widget(Line::from(right).style(Style::new().bg(t.surface)), r);
}

// ── Library ───────────────────────────────────────────────────────────────

fn library(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let hours: i64 = app.visible.iter().map(|&i| app.books[i].runtime_ms).sum::<i64>() / 3_600_000;
    let mut stats = vec![
        stat(&t, tr("books", "libros"), app.visible.len().to_string()),
        stat(&t, tr("hours", "horas"), hours.to_string()),
    ];
    if app.searching || !app.query.is_empty() {
        let cursor = if app.searching { "▏" } else { "" };
        stats.push(stat(&t, tr("search", "busca"), format!("/{}{cursor}", trunc(&app.query, 14))));
    }
    let body = header(f, &t, area, app.filter.label(), stats);

    let card_w = (body.width * 34 / 100).clamp(30, 46);
    let [list_area, _, card_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(2), Constraint::Length(card_w)]).areas(body);
    let list_area = list_area.inner(Margin::new(0, 1));

    let w = list_area.width as usize;
    let author_w = (w / 4).clamp(8, 22);
    let dots = Style::new().fg(t.outline);
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&i| {
            let b = &app.books[i];
            let dl = app.downloads.get(&b.asin);
            let marker = match dl {
                Some(Dl::Queued) => Span::styled("… ", Style::new().fg(t.tertiary)),
                Some(Dl::Running(Progress::Audio { .. })) => Span::styled("↓ ", Style::new().fg(t.tertiary)),
                Some(Dl::Running(Progress::Splitting { .. })) => Span::styled("✂ ", Style::new().fg(t.tertiary)),
                None if b.path.is_some() => Span::styled("● ", Style::new().fg(t.primary)),
                None => Span::raw("  "),
            };
            let progress = match dl {
                Some(Dl::Running(Progress::Audio { done, total })) => {
                    let r = *done as f64 / (*total).max(1) as f64;
                    format!("{} ↓{:>3}%", bar(r, 6), (r * 100.0) as u32)
                }
                _ if b.finished() => format!("{}    ✓", bar(1.0, 6)),
                _ => format!("{} {:>4}", bar(b.percent_complete / 100.0, 6), format!("{:.0}%", b.percent_complete)),
            };
            let author = b.authors.split(',').next().unwrap_or("");
            let right = vec![
                Span::styled(trunc(author, author_w), Style::new().fg(t.on_surface_variant)),
                Span::raw("  "),
                Span::styled(progress, Style::new().fg(t.secondary)),
            ];
            let title_max = w.saturating_sub(right.iter().map(Span::width).sum::<usize>() + 8);
            let left = vec![marker, Span::raw(trunc(&b.title, title_max))];
            ListItem::new(leader(left, right, w, dots))
        })
        .collect();
    let list =
        List::new(items).highlight_style(Style::new().bg(t.primary).fg(t.on_primary).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, list_area, &mut app.list);

    if let Some(b) = app.selected().cloned() {
        book_card(f, app, &t, &b, card_area);
    }
}

/// Inverted card for the selected book.
fn book_card(f: &mut Frame, app: &mut App, t: &Theme, b: &Book, area: Rect) {
    let inv = Style::new().bg(t.primary).fg(t.on_primary);
    let block = Block::bordered().border_type(BorderType::Thick).border_style(inv).style(inv);
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    let w = inner.width as usize;

    let mut lines =
        vec![Line::from(trunc(&b.title.to_uppercase(), w)).style(inv.bold()), Line::from(trunc(&b.authors, w))];
    if !b.series.is_empty() {
        lines.push(Line::from(trunc(&b.series, w)).add_modifier(Modifier::DIM));
    }
    lines.push(Line::from(""));
    let mut rows: Vec<(&str, String)> = vec![(tr("DURATION", "DURACIÓN"), hms(b.runtime_ms))];
    let progress = if b.finished() {
        tr("finished", "terminado").to_owned()
    } else {
        format!("{} {:.0}%", bar(b.percent_complete / 100.0, 8), b.percent_complete)
    };
    rows.push((tr("PROGRESS", "PROGRESO"), progress));
    if let Some(p) = b.position_ms {
        rows.push((tr("POSITION", "POSICIÓN"), hms(p)));
    }
    if let Some(q) = &b.content_format {
        rows.push((tr("QUALITY", "CALIDAD"), q.clone()));
    }
    let n = rows.len();
    for (i, (k, v)) in rows.into_iter().enumerate() {
        lines.push(kv(t, i + 1 == n, k, vec![Span::raw(v)], true));
    }
    lines.push(Line::from(""));
    let chip = |s: &str| Span::raw(format!("[{s}] "));
    let place = if b.path.is_some() { tr("downloaded", "descargado") } else { tr("in the cloud", "en la nube") };
    let mut chips = vec![chip(place)];
    if b.in_progress() {
        chips.push(chip(tr("in progress", "en curso")));
    }
    lines.push(Line::from(chips));
    lines.push(Line::from(""));
    let action = if b.path.is_some() {
        tr("[⏎] Listen", "[⏎] Escuchar")
    } else {
        tr("[⏎] Listen  [D] Download", "[⏎] Escuchar  [D] Descargar")
    };
    lines.push(Line::from(action).style(inv.bold()));

    let text_h = lines.len() as u16;
    let [cover, _, text] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Length(text_h)]).areas(inner);
    render_cover(f, &mut app.cover, &b.asin, cover);
    f.render_widget(Paragraph::new(lines), text);
}

// ── Player ────────────────────────────────────────────────────────────────

fn player(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let Some(p) = &app.player else { return };
    let files = &p.session.files;
    let (idx, in_chapter) = locate(files, p.session.pos);
    let ch = &files[idx];
    let end = p.session.end_ms().max(1);

    let timer = match p.timer {
        Timer::Off => tr("off", "no").to_owned(),
        Timer::At(at) => hms(at.saturating_duration_since(Instant::now()).as_millis() as i64),
        Timer::EndOfChapter(_) => tr("end of ch.", "fin cap.").to_owned(),
    };
    let stats = vec![
        stat(&t, tr("chapter", "capítulo"), format!("{}/{}", idx + 1, files.len())),
        stat(&t, tr("speed", "velocidad"), format!("{:.1}×", p.settings.speed)),
        stat(&t, tr("volume", "volumen"), p.settings.volume.to_string()),
        stat(&t, tr("sleep timer", "temporizador"), timer),
    ];
    let body = header(f, &t, area, &hms(p.session.pos), stats);

    let [top, chapters_area] = Layout::vertical([Constraint::Length(14), Constraint::Min(0)]).areas(body);
    let top = top.inner(Margin::new(0, 1));
    // Cells are about 1:2, so a square cover is twice as wide as it is tall.
    let [cover_area, info_area] =
        Layout::horizontal([Constraint::Length(top.height * 2 + 1), Constraint::Min(0)]).spacing(3).areas(top);

    let info_block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(t.outline)).title(
        Line::styled(format!(" {} ", tr("Now playing", "Reproduciendo")), Style::new().fg(t.primary).bold()).centered(),
    );
    let inner = info_block.inner(info_area).inner(Margin::new(1, 0));
    f.render_widget(info_block, info_area);
    let w = inner.width as usize;
    let bar_cells = w.saturating_sub(34).clamp(6, 40);
    let (state, state_style) = if p.session.paused {
        (tr("⏸ paused", "⏸ en pausa"), Style::new().fg(t.tertiary))
    } else {
        (tr("▶ playing", "▶ reproduciendo"), Style::new().fg(t.primary))
    };
    let sync = match p.synced {
        None => Span::styled(tr("not sent yet", "sin enviar aún"), Style::new().fg(t.on_surface_variant)),
        Some(true) => Span::styled(tr("⟳ synced", "⟳ sincronizado"), Style::new().fg(t.primary)),
        Some(false) => Span::styled(tr("⟳ pending", "⟳ pendiente"), Style::new().fg(t.error)),
    };
    let ratio_ch = in_chapter as f64 / ch.length_ms.max(1) as f64;
    let ratio_book = p.session.pos as f64 / end as f64;
    let text_w = w.saturating_sub(13);
    let (book_k, chapter_k) = (tr("BOOK", "LIBRO"), tr("CHAPTER", "CAPÍTULO"));
    let lines = vec![
        kv(&t, false, book_k, vec![Span::raw(trunc(&p.title, text_w))], false),
        kv(&t, false, tr("AUTHOR", "AUTOR"), vec![Span::raw(trunc(&p.authors, text_w))], false),
        kv(&t, false, chapter_k, vec![Span::raw(trunc(&ch.title, text_w))], false),
        Line::from(""),
        kv(
            &t,
            false,
            chapter_k,
            vec![
                Span::styled(bar(ratio_ch, bar_cells), Style::new().fg(t.secondary)),
                Span::raw(format!(" {} / {}", hms(in_chapter), hms(ch.length_ms))),
            ],
            false,
        ),
        kv(
            &t,
            false,
            book_k,
            vec![
                Span::styled(bar(ratio_book, bar_cells), Style::new().fg(t.secondary)),
                Span::raw(format!(" {:.0}% {} {}", ratio_book * 100.0, tr("of", "de"), hms(end))),
            ],
            false,
        ),
        Line::from(""),
        kv(&t, false, tr("STATUS", "ESTADO"), vec![Span::styled(state, state_style)], false),
        kv(&t, true, "AUDIBLE", vec![sync], false),
    ];
    f.render_widget(Paragraph::new(lines), inner);

    let w = chapters_area.width as usize;
    let items: Vec<ListItem> = files
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let left = vec![
                Span::styled(format!("{:>4}  ", i + 1), Style::new().fg(t.on_surface_variant)),
                Span::raw(trunc(&c.title, w.saturating_sub(20))),
            ];
            let right = vec![Span::styled(hms(c.length_ms), Style::new().fg(t.on_surface_variant))];
            ListItem::new(leader(left, right, w, Style::new().fg(t.outline)))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(t.outline))
                .title(Line::styled(format!(" {} ", tr("CHAPTERS", "CAPÍTULOS")), Style::new().fg(t.primary).bold())),
        )
        .highlight_style(Style::new().bg(t.primary).fg(t.on_primary).add_modifier(Modifier::BOLD));
    let mut state = ListState::default().with_selected(Some(idx));
    f.render_stateful_widget(list, chapters_area, &mut state);

    let asin = p.session.asin.clone();
    render_cover(f, &mut app.cover, &asin, cover_area);
}

// ── Settings ──────────────────────────────────────────────────────────────

/// A Minitel-style form: label, dot leaders and value; actions with their key
/// on the right.
fn settings(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    // The 8x8 font's Ó looks odd at this size, hence no accent.
    let title = tr("Settings", "Configuracion");
    let body = header(f, t, area, title, vec![stat(t, tr("options", "opciones"), super::SETTINGS.to_string())]);
    let [form, _, help] =
        Layout::vertical([Constraint::Length(6), Constraint::Min(0), Constraint::Length(6)]).areas(body);
    let form = form.inner(Margin::new(0, 1));
    let w = form.width as usize;

    let row = |i: usize, label: &str, value: Vec<Span<'static>>| {
        let style = Style::new().fg(t.primary).bold();
        let marker = Span::styled(if app.settings_sel == i { "▸ " } else { "  " }, style);
        leader(vec![marker, Span::styled(format!("{label}:"), style)], value, w, Style::new().fg(t.outline))
    };
    let folder = match &app.editing {
        Some(buf) => vec![Span::styled(format!(" {buf}▏ "), Style::new().bg(t.primary).fg(t.on_primary).bold())],
        None => vec![Span::styled(app.paths.display(&app.paths.library), Style::new().fg(t.on_surface).bold())],
    };
    let lines = vec![
        row(0, tr("DOWNLOAD FOLDER", "CARPETA DE DESCARGAS"), folder),
        Line::styled(
            format!(
                "  {}",
                tr(
                    "new downloads go here; books already downloaded stay where they are",
                    "las descargas nuevas van aquí; los libros ya descargados se quedan donde están"
                )
            ),
            Style::new().fg(t.on_surface_variant),
        ),
        Line::from(""),
        row(1, tr("LANGUAGE", "IDIOMA"), vec![Span::styled(i18n::get().name(), Style::new().fg(t.on_surface).bold())]),
    ];
    f.render_widget(Paragraph::new(lines), form);

    let inspired = Line::styled(
        format!(
            "tbard v{} · {} Libation (github.com/rmcrackan/Libation) {}",
            env!("CARGO_PKG_VERSION"),
            tr("inspired by", "inspirado en"),
            tr("and similar projects", "y proyectos similares")
        ),
        Style::new().fg(t.on_surface_variant),
    );
    let mut help_lines = vec![inspired, separator(t, help.width)];
    help_lines.extend(if app.editing.is_some() {
        actions(t, &[(tr("save the folder", "guardar la carpeta"), "⏎"), (tr("cancel", "cancelar"), "ESC")])
    } else {
        actions(
            t,
            &[
                (tr("change the selected option", "cambiar la opción elegida"), "⏎"),
                (tr("back to the library", "volver a la biblioteca"), "ESC"),
                (tr("quit tbard", "salir de tbard"), "Q"),
            ],
        )
    });
    f.render_widget(Paragraph::new(help_lines), help);
}

/// The language picker, from the settings screen.
fn language(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let body = header(f, t, area, tr("Language", "Idioma"), vec![]);
    let [intro, list_area] = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(body);
    let intro_text = tr("Choose the interface language", "Elige el idioma de la interfaz");
    f.render_widget(
        Paragraph::new(vec![Line::from(""), Line::styled(intro_text, Style::new().fg(t.on_surface).bold())]),
        intro,
    );
    let items: Vec<ListItem> = Lang::ALL
        .iter()
        .map(|l| {
            let current = if *l == i18n::get() { "  ●" } else { "" };
            ListItem::new(Line::from(format!("  {}{current}", l.name())))
        })
        .collect();
    let list = List::new(items)
        .highlight_symbol("▸")
        .highlight_style(Style::new().bg(t.primary).fg(t.on_primary).add_modifier(Modifier::BOLD));
    let mut state = ListState::default().with_selected(Some(app.lang_sel));
    f.render_stateful_widget(list, list_area.inner(Margin::new(0, 1)), &mut state);
}

fn render_cover(f: &mut Frame, cover: &mut Option<(String, StatefulProtocol)>, asin: &str, area: Rect) {
    if let Some((a, proto)) = cover {
        if a == asin {
            f.render_stateful_widget(StatefulImage::default(), area, proto);
        }
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(bar(0.5, 4), "[■■··]");
        assert_eq!(bar(2.0, 3), "[■■■]");
        assert_eq!(trunc("Juramentada", 5), "Jura…");
        assert_eq!(trunc("Dune", 10), "Dune");
        let l = leader(vec![Span::raw("A")], vec![Span::raw("B")], 10, Style::new());
        assert_eq!(l.to_string(), "A ...... B");
        assert_eq!(l.width(), 10);
    }
}
