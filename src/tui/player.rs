use crate::{
    db::Settings,
    player::{locate, Mpv},
    session::Session,
};
use anyhow::Result;
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// Sleep timer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Timer {
    Off,
    At(Instant),
    /// Pause when the chapter with this index ends.
    EndOfChapter(usize),
}

impl Timer {
    /// Off → 15 → 30 → 45 → 60 min → end of chapter → Off.
    pub fn next(self, now: Instant, chapter: usize) -> Self {
        let mins = |m: u64| Self::At(now + Duration::from_secs(m * 60));
        match self {
            Self::Off => mins(15),
            Self::At(t) => match t.saturating_duration_since(now).as_secs().div_ceil(60) {
                0..=15 => mins(30),
                16..=30 => mins(45),
                31..=45 => mins(60),
                _ => Self::EndOfChapter(chapter),
            },
            Self::EndOfChapter(_) => Self::Off,
        }
    }

    /// Is it time to pause?
    pub fn due(self, now: Instant, chapter: usize) -> bool {
        match self {
            Self::Off => false,
            Self::At(t) => now >= t,
            Self::EndOfChapter(c) => chapter != c,
        }
    }
}

/// The book playing inside the TUI.
pub struct Player {
    pub mpv: Mpv,
    pub session: Session,
    pub title: String,
    pub authors: String,
    pub socket: PathBuf,
    pub settings: Settings,
    pub timer: Timer,
    /// Last push to Audible: None = not yet, Some(true) = succeeded.
    pub synced: Option<bool>,
}

impl Player {
    pub fn chapter(&self) -> usize {
        locate(&self.session.files, self.session.pos).0
    }

    pub async fn skip(&mut self, delta_ms: i64) -> Result<()> {
        let target = self.session.pos + delta_ms;
        self.seek(target).await
    }

    /// `[`: to the start of the chapter, or the previous one if already there.
    pub async fn prev_chapter(&mut self) -> Result<()> {
        let (idx, offset) = locate(&self.session.files, self.session.pos);
        let idx = if offset > 3_000 { idx } else { idx.saturating_sub(1) };
        self.seek(self.session.files[idx].start_ms).await
    }

    pub async fn next_chapter(&mut self) -> Result<()> {
        let idx = self.chapter() + 1;
        match self.session.files.get(idx) {
            Some(f) => self.seek(f.start_ms).await,
            None => Ok(()),
        }
    }

    async fn seek(&mut self, pos: i64) -> Result<()> {
        self.mpv.seek_to(&self.session.files, pos).await?;
        self.session.pos = pos.clamp(0, self.session.end_ms());
        Ok(())
    }

    pub async fn speed(&mut self, delta: f64) -> Result<()> {
        let s = ((self.settings.speed + delta) * 10.0).round() / 10.0;
        self.settings.speed = s.clamp(0.5, 3.0);
        self.mpv.set("speed", json!(self.settings.speed)).await
    }

    pub async fn volume(&mut self, delta: i64) -> Result<()> {
        self.settings.volume = (self.settings.volume + delta).clamp(0, 130);
        self.mpv.set("volume", json!(self.settings.volume)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_cycle_and_due() {
        let now = Instant::now();
        let t = Timer::Off.next(now, 3);
        assert_eq!(t, Timer::At(now + Duration::from_secs(15 * 60)));
        let t = t.next(now, 3).next(now, 3).next(now, 3);
        assert_eq!(t, Timer::At(now + Duration::from_secs(60 * 60)));
        let t = t.next(now, 3);
        assert_eq!(t, Timer::EndOfChapter(3));
        assert!(!t.due(now, 3));
        assert!(t.due(now, 4));
        assert_eq!(t.next(now, 3), Timer::Off);
        assert!(Timer::At(now).due(now, 0));
    }
}
