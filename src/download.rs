use crate::{
    audible::{client::Client, content, content::Chapter},
    db::Book,
};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::{fs, io::AsyncWriteExt, process::Command};

/// Un archivo de capítulo y su posición dentro del libro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChapterFile {
    pub file: String,
    pub title: String,
    pub start_ms: i64,
    pub length_ms: i64,
}

pub const INDEX: &str = "chapters.json";

/// Descarga `url` en `dest`. Si la conexión se corta, reintenta hasta 5 veces
/// continuando desde lo ya bajado en `dest.part`.
pub async fn fetch(client: &Client, url: &str, dest: &Path, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let mut attempt = 0;
    loop {
        match fetch_once(client, url, dest, &mut progress).await {
            Err(e) if attempt < 5 => {
                attempt += 1;
                tracing::warn!("descarga de {} interrumpida ({e:#}), reintento {attempt}", dest.display());
                tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
            }
            res => return res,
        }
    }
}

async fn fetch_once(client: &Client, url: &str, dest: &Path, progress: &mut impl FnMut(u64, u64)) -> Result<()> {
    if fs::try_exists(dest).await? {
        return Ok(());
    }
    let part = dest.with_extension("part");
    let have = fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    let res = client
        .http
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={have}-"))
        .send()
        .await?
        .error_for_status()?;
    // 206 = el servidor respeta el Range; 200 = empieza de cero.
    let resumed = res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut done = if resumed { have } else { 0 };
    let total = done + res.content_length().unwrap_or(0);
    let mut out = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)
        .await?;
    let mut body = res.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        out.write_all(&chunk).await?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    out.sync_all().await?;
    fs::rename(&part, dest).await?;
    Ok(())
}

pub enum Progress {
    Audio { done: u64, total: u64 },
    Splitting { done: usize, total: usize },
}

pub struct Downloaded {
    pub dir: PathBuf,
    pub acr: String,
    pub content_format: String,
}

/// Descarga un libro en máxima calidad y lo divide por capítulos en
/// `<biblioteca>/<autor>/<título>`.
pub async fn book(
    client: &Client,
    ffmpeg: &Path,
    library: &Path,
    book: &Book,
    mut progress: impl FnMut(Progress),
) -> Result<Downloaded> {
    let lic = content::license(client, &book.asin).await?;
    let dir = book_dir(library, &book.authors, &book.title);
    fs::create_dir_all(&dir).await?;

    let cover = match &book.cover_url {
        Some(url) => {
            let path = dir.join("cover.jpg");
            fetch(client, url, &path, |_, _| {}).await.ok().map(|_| path)
        }
        None => None,
    };
    let aaxc = dir.join(format!("{}.aaxc", book.asin));
    fetch(client, &lic.offline_url, &aaxc, |done, total| progress(Progress::Audio { done, total })).await?;
    let tags = Tags { album: &book.title, artist: &book.authors, cover: cover.as_deref() };
    split(ffmpeg, &aaxc, Some((&lic.key, &lic.iv)), &lic.chapters, &tags, &dir, |done, total| {
        progress(Progress::Splitting { done, total })
    })
    .await?;
    // Solo se borra el AAXC cuando todos los capítulos están listos.
    fs::remove_file(&aaxc).await?;
    Ok(Downloaded { dir, acr: lic.acr, content_format: lic.content_format })
}

/// Metadata written into every chapter file.
pub struct Tags<'a> {
    pub album: &'a str,
    pub artist: &'a str,
    /// Embedded as cover art.
    pub cover: Option<&'a Path>,
}

/// Corta `input` en un archivo `.m4b` por capítulo, sin recodificar, y escribe
/// `chapters.json` con los offsets. `key_iv` descifra AAXC al vuelo.
pub async fn split(
    ffmpeg: &Path,
    input: &Path,
    key_iv: Option<(&str, &str)>,
    chapters: &[Chapter],
    tags: &Tags<'_>,
    out_dir: &Path,
    mut on_chapter: impl FnMut(usize, usize),
) -> Result<Vec<ChapterFile>> {
    fs::create_dir_all(out_dir).await?;
    let total = chapters.len();
    let width = total.to_string().len().max(3);
    let mut files = Vec::with_capacity(total);
    for (i, ch) in chapters.iter().enumerate() {
        let file = format!("{:0width$} - {}.m4b", i + 1, sanitize(&ch.title));
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-nostdin", "-loglevel", "error", "-y"]);
        if let Some((key, iv)) = key_iv {
            cmd.args(["-audible_key", key, "-audible_iv", iv]);
        }
        cmd.args(["-ss", &secs(ch.start_ms), "-t", &secs(ch.length_ms), "-i"]).arg(input);
        if let Some(cover) = tags.cover {
            cmd.arg("-i").arg(cover).args(["-map", "1:v", "-disposition:v", "attached_pic"]);
        }
        cmd.args(["-map", "0:a", "-c", "copy", "-map_metadata", "-1", "-map_chapters", "-1"])
            .args(["-metadata", &format!("title={}", ch.title)])
            .args(["-metadata", &format!("track={}/{total}", i + 1)])
            .args(["-metadata", &format!("album={}", tags.album)])
            .args(["-metadata", &format!("artist={}", tags.artist)])
            .args(["-metadata", &format!("album_artist={}", tags.artist)])
            .args(["-metadata", "genre=Audiobook", "-f", "mp4"])
            .arg(out_dir.join(&file));
        let out = cmd.output().await.context("no se pudo ejecutar ffmpeg")?;
        if !out.status.success() {
            bail!("ffmpeg falló en «{}»: {}", ch.title, String::from_utf8_lossy(&out.stderr));
        }
        files.push(ChapterFile { file, title: ch.title.clone(), start_ms: ch.start_ms, length_ms: ch.length_ms });
        on_chapter(i + 1, total);
    }
    fs::write(out_dir.join(INDEX), serde_json::to_vec_pretty(&files)?).await?;
    Ok(files)
}

pub async fn load_index(dir: &Path) -> Result<Vec<ChapterFile>> {
    let raw = fs::read(dir.join(INDEX)).await.with_context(|| format!("falta {INDEX} en {}", dir.display()))?;
    Ok(serde_json::from_slice(&raw)?)
}

/// Carpeta del libro: `<biblioteca>/<autor>/<título>`.
pub fn book_dir(library: &Path, authors: &str, title: &str) -> PathBuf {
    let author = authors.split(',').next().unwrap_or("Desconocido");
    library.join(sanitize(author)).join(sanitize(title))
}

fn sanitize(s: &str) -> String {
    let s: String = s.chars().map(|c| if c == '/' || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_start_matches('.');
    if s.is_empty() { "_".into() } else { s.into() }
}

fn secs(ms: i64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(sanitize(" AC/DC: vida "), "AC_DC: vida");
        assert_eq!(sanitize(".."), "_");
        assert_eq!(secs(1_401_275), "1401.275");
        assert_eq!(
            book_dir(Path::new("/b"), "Brandon Sanderson, Otro", "Juramentada"),
            Path::new("/b/Brandon Sanderson/Juramentada")
        );
    }

    /// Necesita ffmpeg en el PATH (devShell); en el sandbox de Nix se salta.
    #[tokio::test]
    async fn split_synthetic_book() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("ffmpeg no disponible, se salta");
            return;
        }
        let dir = std::env::temp_dir().join(format!("tbard-split-{}", std::process::id()));
        fs::create_dir_all(&dir).await.unwrap();
        let src = dir.join("book.m4a");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=22050:duration=9"])
            .args(["-c:a", "aac", "-b:a", "64k"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(ok.success());

        let chapters = [
            Chapter { title: "Uno".into(), start_ms: 0, length_ms: 2_000 },
            Chapter { title: "Dos".into(), start_ms: 2_000, length_ms: 3_000 },
            Chapter { title: "Tres".into(), start_ms: 5_000, length_ms: 4_000 },
        ];
        let out = dir.join("out");
        let files =
            split(Path::new("ffmpeg"), &src, None, &chapters, &Tags { album: "Libro", artist: "Autora", cover: None }, &out, |_, _| {})
                .await
                .unwrap();
        assert_eq!(load_index(&out).await.unwrap(), files);

        for (f, ch) in files.iter().zip(&chapters) {
            let probe = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
                .arg(out.join(&f.file))
                .output()
                .unwrap();
            let dur: f64 = String::from_utf8_lossy(&probe.stdout).trim().parse().unwrap();
            let diff = (dur * 1000.0 - ch.length_ms as f64).abs();
            assert!(diff < 100.0, "{}: {dur}s vs {}ms", f.file, ch.length_ms);
        }
        fs::remove_dir_all(&dir).await.unwrap();
    }
}
