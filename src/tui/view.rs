use super::{player::Timer, App, Dl, View};
use crate::{download::Progress, hms, player::locate};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, LineGauge, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use ratatui_image::{protocol::StatefulProtocol, StatefulImage};
use std::time::{Duration, Instant};

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = app.theme.clone();
    f.render_widget(Block::new().style(Style::new().bg(t.surface).fg(t.on_surface)), f.area());

    let now_playing = app.view == View::Library && app.player.is_some();
    let [main, playing, status, help] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(now_playing as u16),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    match app.view {
        View::Library => library(f, app, main),
        View::Player => player(f, app, main),
    }
    if now_playing {
        if let Some(p) = &app.player {
            let icon = if p.session.paused { "⏸" } else { "▶" };
            let line = Line::from(vec![
                Span::styled(format!(" {icon} "), Style::new().fg(t.on_primary).bg(t.primary)),
                Span::raw(format!(" {} · {} / {} ", p.title, hms(p.session.pos), hms(p.session.end_ms()))),
                Span::styled("tab: reproductor", Style::new().fg(t.on_surface_variant)),
            ]);
            f.render_widget(line, playing);
        }
    }

    let msg = match &app.status {
        Some((m, at)) if at.elapsed() < Duration::from_secs(6) => Span::raw(format!(" {m}")),
        _ => match app.online {
            Some(true) => Span::styled(" ● conectado a Audible", Style::new().fg(t.primary)),
            Some(false) => Span::styled(" ● sin conexión (las posiciones se envían luego)", Style::new().fg(t.error)),
            None => Span::styled(" ○ conectando…", Style::new().fg(t.on_surface_variant)),
        },
    };
    f.render_widget(Line::from(msg), status);

    let keys = match (app.searching, &app.view) {
        (true, _) => "escribe para buscar · ⏎ aceptar · esc borrar",
        (_, View::Library) => "/ buscar · f filtro · d descargar · ⏎ reproducir · r actualizar · tab reproductor · q salir",
        (_, View::Player) => {
            "espacio pausa · ←/→ 30 s · [/] capítulo · +/- velocidad · 9/0 volumen · t temporizador · s parar · tab biblioteca · q salir"
        }
    };
    f.render_widget(Line::styled(format!(" {keys}"), Style::new().fg(t.on_surface_variant)), help);

    if let Some(ask) = &app.ask {
        let area = centered(f.area(), 52, 7);
        f.render_widget(Clear, area);
        let text = vec![
            Line::from("Las posiciones no coinciden:"),
            Line::from(format!("  aquí (l):     {}", hms(ask.local))),
            Line::from(format!("  Audible (a):  {}", hms(ask.remote))),
            Line::styled("l / a (⏎) / esc", Style::new().fg(t.on_surface_variant)),
        ];
        let block = block(&t, " ¿Desde dónde sigo? ").border_style(Style::new().fg(t.tertiary));
        f.render_widget(Paragraph::new(text).block(block).style(Style::new().bg(t.surface)), area);
    }
}

fn block<'a>(t: &super::Theme, title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.outline))
        .title(title.into().fg(t.primary).bold())
}

fn library(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let [list_area, detail] = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);

    let mut title = format!(" tbard · {} · {} libros ", app.filter.label(), app.visible.len());
    if app.searching || !app.query.is_empty() {
        title.push_str(&format!("· /{}{} ", app.query, if app.searching { "▏" } else { "" }));
    }
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&i| {
            let b = &app.books[i];
            let marker = match app.downloads.get(&b.asin) {
                Some(Dl::Queued) => Span::styled(" … ", Style::new().fg(t.tertiary)),
                Some(Dl::Running(Progress::Audio { done, total })) => {
                    Span::styled(format!("{:>3}%", done * 100 / (*total).max(1)), Style::new().fg(t.tertiary))
                }
                Some(Dl::Running(Progress::Splitting { .. })) => Span::styled(" ✂ ", Style::new().fg(t.tertiary)),
                None if b.path.is_some() => Span::styled(" ● ", Style::new().fg(t.primary)),
                None => Span::raw("   "),
            };
            let mut spans = vec![marker, Span::raw(" "), Span::raw(b.title.clone())];
            spans.push(Span::styled(format!(" — {}", b.authors), Style::new().fg(t.on_surface_variant)));
            if b.finished() {
                spans.push(Span::styled("  ✓", Style::new().fg(t.secondary)));
            } else if b.percent_complete > 0.0 {
                spans.push(Span::styled(format!("  {:.0}%", b.percent_complete), Style::new().fg(t.secondary)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(block(&t, title))
        .highlight_style(Style::new().bg(t.primary).fg(t.on_primary).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, list_area, &mut app.list);

    let detail_block = block(&t, "");
    let inner = detail_block.inner(detail);
    f.render_widget(detail_block, detail);
    let Some(b) = app.selected().cloned() else { return };
    let [cover_area, text_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(9)]).areas(inner);
    render_cover(f, &mut app.cover, &b.asin, cover_area);

    let mut lines = vec![
        Line::from(b.title.clone()).bold().fg(t.primary),
        Line::from(b.authors.clone()),
    ];
    if !b.series.is_empty() {
        lines.push(Line::styled(b.series.clone(), Style::new().fg(t.on_surface_variant)));
    }
    lines.push(Line::from(format!("Duración: {}", hms(b.runtime_ms))));
    if b.percent_complete > 0.0 {
        lines.push(Line::from(format!("Progreso en Audible: {:.0}%", b.percent_complete)));
    }
    if let Some(p) = b.position_ms {
        lines.push(Line::from(format!("Posición aquí: {}", hms(p))));
    }
    lines.push(match (&b.path, &b.content_format) {
        (Some(_), Some(q)) => Line::styled(format!("Descargado · {q}"), Style::new().fg(t.primary)),
        (Some(_), None) => Line::styled("Descargado", Style::new().fg(t.primary)),
        _ => Line::styled("Sin descargar", Style::new().fg(t.on_surface_variant)),
    });
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), text_area);
}

fn player(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let Some(p) = &app.player else { return };
    let files = &p.session.files;
    let (idx, in_chapter) = locate(files, p.session.pos);
    let ch = &files[idx];
    let end = p.session.end_ms().max(1);

    let [top, chapters_area] = Layout::vertical([Constraint::Length(12), Constraint::Min(0)]).areas(area);
    let top_block = block(&t, " reproduciendo ");
    let inner = top_block.inner(top);
    f.render_widget(top_block, top);
    // Las celdas miden ~1:2, así que la portada cuadrada ocupa el doble de ancho que de alto.
    let [cover_area, info] =
        Layout::horizontal([Constraint::Length(inner.height * 2 + 1), Constraint::Min(0)]).areas(inner);

    let state = if p.session.paused { "⏸ en pausa" } else { "▶ reproduciendo" };
    let timer = match p.timer {
        Timer::Off => String::new(),
        Timer::At(at) => format!(" · ⏾ {}", hms(at.saturating_duration_since(Instant::now()).as_millis() as i64)),
        Timer::EndOfChapter(_) => " · ⏾ fin de capítulo".into(),
    };
    let sync = match p.synced {
        None => Span::styled(" · sin enviar aún", Style::new().fg(t.on_surface_variant)),
        Some(true) => Span::styled(" · ⟳ sincronizado", Style::new().fg(t.primary)),
        Some(false) => Span::styled(" · ⟳ pendiente", Style::new().fg(t.error)),
    };
    let [l_title, l_author, l_chapter, _, g_chapter, g_book, _, l_state] =
        Layout::vertical([Constraint::Length(1); 8]).areas(info);
    f.render_widget(Line::from(p.title.clone()).bold().fg(t.primary), l_title);
    f.render_widget(Line::from(p.authors.clone()), l_author);
    f.render_widget(
        Line::from(format!("Capítulo {}/{} · {}", idx + 1, files.len(), ch.title)).fg(t.secondary),
        l_chapter,
    );
    let gauge = |ratio: f64, label: String| {
        LineGauge::default()
            .ratio(ratio.clamp(0.0, 1.0))
            .label(label)
            .filled_style(Style::new().fg(t.primary))
            .unfilled_style(Style::new().fg(t.outline))
    };
    f.render_widget(
        gauge(
            in_chapter as f64 / ch.length_ms.max(1) as f64,
            format!("{} / {} ", hms(in_chapter), hms(ch.length_ms)),
        ),
        g_chapter,
    );
    f.render_widget(
        gauge(
            p.session.pos as f64 / end as f64,
            format!("{} / {} ({:.0}%) ", hms(p.session.pos), hms(end), p.session.pos as f64 * 100.0 / end as f64),
        ),
        g_book,
    );
    f.render_widget(
        Line::from(vec![
            Span::raw(format!("{state} · {:.1}× · vol {}{timer}", p.settings.speed, p.settings.volume)),
            sync,
        ]),
        l_state,
    );

    let items: Vec<ListItem> = files
        .iter()
        .enumerate()
        .map(|(i, c)| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:>4} ", i + 1), Style::new().fg(t.on_surface_variant)),
                Span::raw(c.title.clone()),
                Span::styled(format!("  {}", hms(c.length_ms)), Style::new().fg(t.on_surface_variant)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(block(&t, " capítulos "))
        .highlight_style(Style::new().bg(t.primary).fg(t.on_primary).add_modifier(Modifier::BOLD));
    let mut state = ListState::default().with_selected(Some(idx));
    f.render_stateful_widget(list, chapters_area, &mut state);

    let asin = p.session.asin.clone();
    render_cover(f, &mut app.cover, &asin, cover_area);
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
