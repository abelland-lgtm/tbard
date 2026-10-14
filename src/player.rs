//! mpv as the audio player, controlled through its JSON IPC: a Unix socket on
//! Linux/macOS and a named pipe on Windows.

use crate::download::ChapterFile;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

/// How mpv is launched.
pub struct Options {
    /// The CLI hands the terminal to mpv; the TUI does not.
    pub terminal: bool,
    pub speed: f64,
    pub volume: i64,
}

/// Where mpv listens for IPC. On Unix a socket inside the private runtime
/// directory, so other users cannot control playback.
pub fn ipc_path(runtime: &Path) -> PathBuf {
    let name = format!("tbard-{}", std::process::id());
    if cfg!(windows) {
        PathBuf::from(format!(r"\\.\pipe\{name}"))
    } else {
        runtime.join(format!("{name}.sock"))
    }
}

type Reader = BufReader<Box<dyn AsyncRead + Unpin + Send>>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

async fn connect(path: &Path) -> std::io::Result<(Reader, Writer)> {
    #[cfg(unix)]
    let (r, w) = tokio::io::split(tokio::net::UnixStream::connect(path).await?);
    #[cfg(windows)]
    let (r, w) = tokio::io::split(tokio::net::windows::named_pipe::ClientOptions::new().open(path)?);
    Ok((BufReader::new(Box::new(r)), Box::new(w)))
}

/// mpv's IPC protocol: one JSON object per line, replies matched by
/// `request_id`, with events interleaved.
struct Ipc {
    reader: Reader,
    writer: Writer,
    next_id: u64,
}

impl Ipc {
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
                bail!("mpv closed the IPC connection");
            }
            let msg: Value = serde_json::from_str(&buf)?;
            if msg["request_id"].as_u64() == Some(id) {
                if msg["error"].as_str() != Some("success") {
                    bail!("mpv: {}", msg["error"]);
                }
                return Ok(msg["data"].clone());
            }
        }
    }
}

pub struct Mpv {
    child: Child,
    ipc: Ipc,
}

impl Mpv {
    /// Starts mpv with the chapters as a playlist, at `start_ms` into the book.
    pub async fn spawn(
        mpv: &Path,
        ipc: &Path,
        dir: &Path,
        files: &[ChapterFile],
        start_ms: i64,
        opts: &Options,
    ) -> Result<Self> {
        let (idx, offset_ms) = locate(files, start_ms);
        if cfg!(unix) {
            let _ = std::fs::remove_file(ipc);
        }
        let mut cmd = Command::new(mpv);
        cmd.args(["--no-video", "--force-window=no", "--idle=no"])
            .arg(format!("--input-ipc-server={}", ipc.display()))
            .arg(format!("--playlist-start={idx}"))
            .arg(format!("--speed={}", opts.speed))
            .arg(format!("--volume={}", opts.volume));
        if !opts.terminal {
            cmd.arg("--no-terminal").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        }
        for (i, f) in files.iter().enumerate() {
            if i == idx {
                // `--start` only for the first file: options between --{ and --} are per-file.
                cmd.args(["--{", &format!("--start={}", offset_ms as f64 / 1000.0)]).arg(dir.join(&f.file)).arg("--}");
            } else {
                cmd.arg(dir.join(&f.file));
            }
        }
        let mut child = cmd.kill_on_drop(true).spawn().with_context(|| format!("could not run {}", mpv.display()))?;

        for _ in 0..50 {
            if let Ok((reader, writer)) = connect(ipc).await {
                return Ok(Self { child, ipc: Ipc { reader, writer, next_id: 1 } });
            }
            if child.try_wait()?.is_some() {
                bail!("mpv exited on startup");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!("mpv did not open its IPC endpoint")
    }

    async fn command(&mut self, args: Value) -> Result<Value> {
        self.ipc.command(args).await
    }

    pub async fn get(&mut self, prop: &str) -> Result<Value> {
        self.command(json!(["get_property", prop])).await
    }

    /// Position in the book in ms: the current chapter's offset plus the time into it.
    pub async fn position(&mut self, files: &[ChapterFile]) -> Result<i64> {
        let idx = self.get("playlist-pos").await?.as_i64().context("no playlist-pos")?;
        let secs = self.get("time-pos").await?.as_f64().unwrap_or(0.0);
        let ch = files.get(idx as usize).context("playlist-pos out of range")?;
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

    /// Jumps to a position in the book, switching chapter file if needed.
    pub async fn seek_to(&mut self, files: &[ChapterFile], pos_ms: i64) -> Result<()> {
        let end = files.last().map_or(0, |f| f.start_ms + f.length_ms);
        let (idx, offset) = locate(files, pos_ms.clamp(0, (end - 1000).max(0)));
        let secs = offset as f64 / 1000.0;
        if self.get("playlist-pos").await?.as_i64() != Some(idx as i64) {
            self.command(json!(["playlist-play-index", idx])).await?;
            // Seeking only works once the new file has loaded.
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

/// Position in the book → (chapter index, ms into that chapter).
pub fn locate(files: &[ChapterFile], pos_ms: i64) -> (usize, i64) {
    let idx = files.iter().rposition(|f| f.start_ms <= pos_ms).unwrap_or(0);
    (idx, (pos_ms - files.get(idx).map_or(0, |f| f.start_ms)).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    /// Plays mpv's side of the IPC on `stream`: answers each request with an
    /// event first (which the client must skip) and then the reply.
    async fn fake_mpv<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(mut stream: S) {
        let mut buf = vec![0u8; 4096];
        let mut pending = String::new();
        while let Ok(n @ 1..) = stream.read(&mut buf).await {
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(end) = pending.find('\n') {
                let req: Value = serde_json::from_str(&pending[..end]).unwrap();
                pending.drain(..=end);
                let id = req["request_id"].as_u64().unwrap();
                let data = match req["command"][1].as_str() {
                    Some("time-pos") => json!(12.5),
                    _ => Value::Null,
                };
                let out = format!(
                    "{}\n{}\n",
                    json!({"event": "property-change", "name": "pause"}),
                    json!({"request_id": id, "error": "success", "data": data})
                );
                stream.write_all(out.as_bytes()).await.unwrap();
            }
        }
    }

    async fn roundtrip(path: &Path) {
        let (reader, writer) = connect(path).await.unwrap();
        let mut ipc = Ipc { reader, writer, next_id: 1 };
        assert_eq!(ipc.command(json!(["get_property", "time-pos"])).await.unwrap(), json!(12.5));
        assert_eq!(ipc.command(json!(["set_property", "pause", true])).await.unwrap(), Value::Null);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ipc_over_unix_socket() {
        let path = std::env::temp_dir().join(format!("tbard-ipc-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move { fake_mpv(listener.accept().await.unwrap().0).await });
        roundtrip(&path).await;
        std::fs::remove_file(&path).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn ipc_over_named_pipe() {
        let path = ipc_path(Path::new(""));
        let server = tokio::net::windows::named_pipe::ServerOptions::new().create(&path).unwrap();
        tokio::spawn(async move {
            server.connect().await.unwrap();
            fake_mpv(server).await
        });
        roundtrip(&path).await;
    }
}
