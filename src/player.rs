use crate::download::ChapterFile;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixStream,
    },
    process::{Child, Command},
};

/// Cómo se lanza mpv.
pub struct Options {
    /// La CLI le deja la terminal a mpv; la TUI no.
    pub terminal: bool,
    pub speed: f64,
    pub volume: i64,
}

/// mpv controlado por su socket IPC (JSON por líneas).
pub struct Mpv {
    child: Child,
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
}

impl Mpv {
    /// Lanza mpv con los capítulos como playlist, empezando en `start_ms` del libro.
    pub async fn spawn(socket: &Path, dir: &Path, files: &[ChapterFile], start_ms: i64, opts: &Options) -> Result<Self> {
        let (idx, offset_ms) = locate(files, start_ms);
        let _ = std::fs::remove_file(socket);
        let mut cmd = Command::new("mpv");
        cmd.args(["--no-video", "--force-window=no", "--idle=no"])
            .arg(format!("--input-ipc-server={}", socket.display()))
            .arg(format!("--playlist-start={idx}"))
            .arg(format!("--speed={}", opts.speed))
            .arg(format!("--volume={}", opts.volume));
        if !opts.terminal {
            cmd.arg("--no-terminal").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        }
        for (i, f) in files.iter().enumerate() {
            if i == idx {
                // --start solo para el primer archivo: entre --{ y --} es opción por archivo.
                cmd.args(["--{", &format!("--start={}", offset_ms as f64 / 1000.0)])
                    .arg(dir.join(&f.file))
                    .arg("--}");
            } else {
                cmd.arg(dir.join(&f.file));
            }
        }
        let mut child = cmd.kill_on_drop(true).spawn().context("no se pudo ejecutar mpv")?;

        let mut stream = None;
        for _ in 0..50 {
            if let Ok(s) = UnixStream::connect(socket).await {
                stream = Some(s);
                break;
            }
            if child.try_wait()?.is_some() {
                bail!("mpv se cerró al arrancar");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let (r, writer) = stream.context("mpv no abrió el socket IPC")?.into_split();
        Ok(Self { child, reader: BufReader::new(r), writer, next_id: 1 })
    }

    async fn command(&mut self, args: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({ "command": args, "request_id": id }).to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        let mut buf = String::new();
        loop {
            buf.clear();
            if self.reader.read_line(&mut buf).await? == 0 {
                bail!("mpv cerró el socket");
            }
            let msg: Value = serde_json::from_str(&buf)?;
            // Los eventos se intercalan con las respuestas.
            if msg["request_id"].as_u64() == Some(id) {
                if msg["error"].as_str() != Some("success") {
                    bail!("mpv: {}", msg["error"]);
                }
                return Ok(msg["data"].clone());
            }
        }
    }

    pub async fn get(&mut self, prop: &str) -> Result<Value> {
        self.command(json!(["get_property", prop])).await
    }

    /// Posición del libro en ms, sumando el offset del capítulo actual.
    pub async fn position(&mut self, files: &[ChapterFile]) -> Result<i64> {
        let idx = self.get("playlist-pos").await?.as_i64().context("sin playlist-pos")?;
        let secs = self.get("time-pos").await?.as_f64().unwrap_or(0.0);
        let ch = files.get(idx as usize).context("playlist-pos fuera de rango")?;
        Ok(ch.start_ms + (secs * 1000.0) as i64)
    }

    pub async fn paused(&mut self) -> Result<bool> {
        Ok(self.get("pause").await?.as_bool().unwrap_or(false))
    }

    pub async fn set(&mut self, prop: &str, value: Value) -> Result<()> {
        self.command(json!(["set_property", prop, value])).await.map(drop)
    }

    pub async fn toggle_pause(&mut self) -> Result<()> {
        self.command(json!(["cycle", "pause"])).await.map(drop)
    }

    pub async fn quit(&mut self) -> Result<()> {
        self.command(json!(["quit"])).await.map(drop)
    }

    /// Salta a una posición del libro, cambiando de capítulo si hace falta.
    pub async fn seek_to(&mut self, files: &[ChapterFile], pos_ms: i64) -> Result<()> {
        let end = files.last().map_or(0, |f| f.start_ms + f.length_ms);
        let (idx, offset) = locate(files, pos_ms.clamp(0, (end - 1000).max(0)));
        let secs = offset as f64 / 1000.0;
        if self.get("playlist-pos").await?.as_i64() != Some(idx as i64) {
            self.command(json!(["playlist-play-index", idx])).await?;
            // El seek solo vale cuando el archivo nuevo ya cargó.
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if self.get("playlist-pos").await?.as_i64() == Some(idx as i64) && self.get("time-pos").await.is_ok() {
                    break;
                }
            }
        }
        self.command(json!(["seek", secs, "absolute"])).await.map(drop)
    }

    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }
}

/// Posición del libro → (índice de capítulo, ms dentro del capítulo).
pub fn locate(files: &[ChapterFile], pos_ms: i64) -> (usize, i64) {
    let idx = files.iter().rposition(|f| f.start_ms <= pos_ms).unwrap_or(0);
    (idx, (pos_ms - files.get(idx).map_or(0, |f| f.start_ms)).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<ChapterFile> {
        [(0, 21_175), (21_175, 11_795), (32_970, 1_368_305)]
            .into_iter()
            .enumerate()
            .map(|(i, (start_ms, length_ms))| ChapterFile {
                file: format!("{i}.m4b"),
                title: String::new(),
                start_ms,
                length_ms,
            })
            .collect()
    }

    #[test]
    fn locate_positions() {
        let f = files();
        assert_eq!(locate(&f, 0), (0, 0));
        assert_eq!(locate(&f, 21_174), (0, 21_174));
        assert_eq!(locate(&f, 21_175), (1, 0));
        assert_eq!(locate(&f, 100_000), (2, 67_030));
        assert_eq!(locate(&[], 5), (0, 5));
    }
}
