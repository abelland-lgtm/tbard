//! Cuándo guardar y cuándo enviar la posición durante una reproducción.
//! Es lógica pura: la CLI y la TUI ejecutan las acciones a su manera.

use crate::download::ChapterFile;
use std::time::{Duration, Instant};

/// Cada cuánto se guarda en local y se envía a Audible.
const SAVE_EVERY: Duration = Duration::from_secs(5);
const PUSH_EVERY: Duration = Duration::from_secs(120);
/// A menos de esto del final, el libro cuenta como terminado.
const FINISH_MARGIN_MS: i64 = 30_000;

pub struct Session {
    pub asin: String,
    pub acr: String,
    pub files: Vec<ChapterFile>,
    pub pos: i64,
    pub paused: bool,
    pushed: i64,
    last_save: Instant,
    last_push: Instant,
}

#[derive(Debug, Default, PartialEq)]
pub struct Actions {
    pub save: bool,
    /// Posición a enviar a Audible.
    pub push: Option<i64>,
}

impl Session {
    pub fn new(asin: String, acr: String, files: Vec<ChapterFile>, start: i64, now: Instant) -> Self {
        Self { asin, acr, files, pos: start, paused: false, pushed: start, last_save: now, last_push: now }
    }

    pub fn end_ms(&self) -> i64 {
        self.files.last().map_or(0, |f| f.start_ms + f.length_ms)
    }

    /// Registra el estado de mpv y decide qué hacer.
    pub fn update(&mut self, pos: i64, paused: bool, now: Instant) -> Actions {
        let just_paused = paused && !self.paused;
        (self.pos, self.paused) = (pos, paused);
        let mut act = Actions::default();
        if now - self.last_save >= SAVE_EVERY || just_paused {
            act.save = true;
            self.last_save = now;
        }
        if pos != self.pushed && (just_paused || now - self.last_push >= PUSH_EVERY) {
            act.push = Some(pos);
            (self.pushed, self.last_push) = (pos, now);
        }
        act
    }

    /// Posición final: el final del libro si quedaba menos del margen.
    pub fn finish(&self) -> (i64, bool) {
        let end = self.end_ms();
        let finished = end > 0 && self.pos >= end - FINISH_MARGIN_MS;
        (if finished { end } else { self.pos }, finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(now: Instant) -> Session {
        let files = vec![ChapterFile { file: "1.m4b".into(), title: String::new(), start_ms: 0, length_ms: 600_000 }];
        Session::new("A".into(), "acr".into(), files, 1_000, now)
    }

    #[test]
    fn saves_and_pushes_on_schedule() {
        let t0 = Instant::now();
        let mut s = session(t0);
        assert_eq!(s.update(2_000, false, t0 + Duration::from_secs(1)), Actions::default());
        assert_eq!(s.update(6_000, false, t0 + Duration::from_secs(5)), Actions { save: true, push: None });
        assert_eq!(
            s.update(121_000, false, t0 + Duration::from_secs(120)),
            Actions { save: true, push: Some(121_000) }
        );
    }

    #[test]
    fn pause_pushes_once() {
        let t0 = Instant::now();
        let mut s = session(t0);
        let t = t0 + Duration::from_secs(2);
        assert_eq!(s.update(3_000, true, t), Actions { save: true, push: Some(3_000) });
        // Sigue en pausa: ni se repite el envío.
        assert_eq!(s.update(3_000, true, t + Duration::from_secs(1)), Actions::default());
    }

    #[test]
    fn finish_near_end_marks_finished() {
        let t0 = Instant::now();
        let mut s = session(t0);
        s.update(590_000, false, t0);
        assert_eq!(s.finish(), (600_000, true));
        s.update(100_000, false, t0);
        assert_eq!(s.finish(), (100_000, false));
    }
}
