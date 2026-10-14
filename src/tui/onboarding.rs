//! First run inside the TUI: choose the language, then sign in to Audible.
//! Runs before the main app, which needs credentials to start.

use super::{
    theme::Theme,
    view::{actions, header},
};
use crate::{
    audible::{auth::tld, login},
    config::Paths,
    i18n::{self, tr, Lang},
    secret,
};
use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
};
use futures_util::StreamExt;
use ratatui::{
    layout::{Constraint, Layout, Margin},
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
    DefaultTerminal, Frame,
};

/// Audible stores: code and name in English and Spanish.
const STORES: [(&str, &str, &str); 11] = [
    ("us", "United States", "Estados Unidos"),
    ("uk", "United Kingdom", "Reino Unido"),
    ("ca", "Canada", "Canadá"),
    ("au", "Australia", "Australia"),
    ("in", "India", "India"),
    ("de", "Germany", "Alemania"),
    ("fr", "France", "Francia"),
    ("it", "Italy", "Italia"),
    ("es", "Spain", "España"),
    ("jp", "Japan", "Japón"),
    ("br", "Brazil", "Brasil"),
];

enum Step {
    Language { sel: usize },
    Store { sel: usize },
    Paste { pending: login::Pending, input: String, error: Option<String> },
    Busy,
}

/// Returns `false` if the user quit before finishing.
pub async fn run(terminal: &mut DefaultTerminal, paths: &mut Paths, theme: &Theme) -> Result<bool> {
    let need_login = !secret::exists(&paths.data);
    let mut step = if paths.language.is_none() {
        Step::Language { sel: Lang::ALL.iter().position(|&l| l == Lang::system()).unwrap_or(0) }
    } else if need_login {
        Step::Store { sel: 0 }
    } else {
        return Ok(true);
    };
    // Long sign-in URLs arrive as one paste event instead of hundreds of key presses.
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let res = steps(terminal, paths, theme, &mut step, need_login).await;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    res
}

async fn steps(
    terminal: &mut DefaultTerminal,
    paths: &mut Paths,
    theme: &Theme,
    step: &mut Step,
    need_login: bool,
) -> Result<bool> {
    let mut events = EventStream::new();
    loop {
        terminal.draw(|f| draw(f, theme, step))?;
        let key = match events.next().await {
            Some(Ok(Event::Key(k))) if k.kind == KeyEventKind::Press => k,
            Some(Ok(Event::Paste(text))) => {
                if let Step::Paste { input, .. } = step {
                    input.push_str(text.trim());
                }
                continue;
            }
            Some(Err(e)) => return Err(e.into()),
            None => return Ok(false),
            _ => continue,
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(false);
        }
        let n = |sel: &mut usize, len: usize| match key.code {
            KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(len - 1),
            KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
            _ => {}
        };
        match step {
            Step::Language { sel } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(false),
                KeyCode::Enter => {
                    let lang = Lang::ALL[*sel];
                    i18n::set(lang);
                    paths.set_language(lang)?;
                    if !need_login {
                        return Ok(true);
                    }
                    *step = Step::Store { sel: 0 };
                }
                _ => n(sel, Lang::ALL.len()),
            },
            Step::Store { sel } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(false),
                KeyCode::Enter => {
                    let pending = login::start(STORES[*sel].0)?;
                    let _ = open::that(&pending.url);
                    *step = Step::Paste { pending, input: String::new(), error: None };
                }
                _ => n(sel, STORES.len()),
            },
            Step::Paste { input, .. } if key.code != KeyCode::Enter => match key.code {
                KeyCode::Esc => *step = Step::Store { sel: 0 },
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            },
            Step::Paste { .. } => {
                let Step::Paste { pending, input, .. } = std::mem::replace(step, Step::Busy) else { unreachable!() };
                terminal.draw(|f| draw(f, theme, step))?;
                match login::finish(&reqwest::Client::new(), &pending, &input).await {
                    Ok(auth) => {
                        secret::save(&paths.data, &auth, &secret::OsKeychain)?;
                        return Ok(true);
                    }
                    Err(e) => *step = Step::Paste { pending, input, error: Some(format!("{e:#}")) },
                }
            }
            Step::Busy => {}
        }
    }
}

/// The end of `s`, at most `max` characters, so the cursor stays visible.
fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    let mut out = String::from("…");
    out.extend(s.chars().skip(n + 1 - max));
    out
}

fn draw(f: &mut Frame, t: &Theme, step: &Step) {
    f.render_widget(ratatui::widgets::Block::new().style(Style::new().bg(t.surface).fg(t.on_surface)), f.area());
    let area = f.area().inner(Margin::new(4, 1));
    let selected = Style::new().bg(t.primary).fg(t.on_primary).add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(t.on_surface_variant);

    let (title, intro) = match step {
        Step::Language { .. } => ("Welcome", "Choose your language  ·  Elige tu idioma"),
        Step::Store { .. } => (
            tr("Welcome", "Bienvenida"),
            tr("Which Audible store is your account on?", "¿En qué tienda de Audible está tu cuenta?"),
        ),
        Step::Paste { .. } | Step::Busy => (
            tr("Sign in", "Iniciar sesion"),
            tr(
                "Sign in to Amazon in the browser window that just opened.",
                "Inicia sesión en Amazon en la ventana del navegador que se acaba de abrir.",
            ),
        ),
    };
    let body = header(f, t, area, title, vec![]);
    let [intro_area, main, help] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0), Constraint::Length(4)]).areas(body);
    f.render_widget(Paragraph::new(vec![Line::from(""), Line::from(intro).bold()]), intro_area);

    match step {
        Step::Language { sel } => {
            let items: Vec<ListItem> = Lang::ALL.iter().map(|l| ListItem::new(format!("  {}", l.name()))).collect();
            let list = List::new(items).highlight_symbol("▸").highlight_style(selected);
            f.render_stateful_widget(list, main, &mut ListState::default().with_selected(Some(*sel)));
        }
        Step::Store { sel } => {
            let items: Vec<ListItem> = STORES
                .iter()
                .map(|(code, en, es)| {
                    let name = if i18n::get() == Lang::Es { es } else { en };
                    ListItem::new(format!("  {name:<16} audible.{}", tld(code)))
                })
                .collect();
            let list = List::new(items).highlight_symbol("▸").highlight_style(selected);
            f.render_stateful_widget(list, main, &mut ListState::default().with_selected(Some(*sel)));
        }
        Step::Paste { pending, input, error } => {
            let mut lines = vec![
                Line::from(tr(
                    "When you finish, the browser lands on a blank or \"page not found\" page whose address starts with",
                    "Al terminar, el navegador llega a una página en blanco o de \"página no encontrada\" cuya dirección empieza por",
                )),
                Line::from("https://www.amazon…/ap/maplanding?…").bold(),
                Line::from(tr(
                    "Copy that whole address and paste it here, then press Enter.",
                    "Copia esa dirección completa, pégala aquí y pulsa Enter.",
                )),
                Line::from(""),
                Line::styled(format!(" {}▏ ", tail(input, (main.width as usize).saturating_sub(4))), selected),
                Line::from(""),
            ];
            if let Some(e) = error {
                lines.push(Line::styled(e.clone(), Style::new().fg(t.error)));
                lines.push(Line::from(""));
            }
            lines.push(Line::styled(
                tr("If the browser did not open, use this URL:", "Si el navegador no se abrió, usa esta URL:"),
                dim,
            ));
            lines.push(Line::styled(pending.url.clone(), dim));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), main);
        }
        Step::Busy => {
            f.render_widget(Paragraph::new(tr("Registering this device…", "Registrando este dispositivo…")), main)
        }
    }

    let keys: &[(&str, &str)] = match step {
        Step::Paste { .. } => {
            &[(tr("sign in", "iniciar sesión"), "⏎"), (tr("choose another store", "elegir otra tienda"), "ESC")]
        }
        Step::Busy => &[],
        _ => &[(tr("continue", "continuar"), "⏎"), (tr("quit", "salir"), "ESC")],
    };
    f.render_widget(Paragraph::new(actions(t, keys)), help);
}
