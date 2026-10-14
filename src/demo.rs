//! Demo mode for screenshots: `cargo run --features demo` with `TBARD_DEMO=1`
//! starts the TUI on a library of public-domain classics with generated
//! covers, without an account or network access. Point the XDG/AppData
//! directories at a scratch folder first: it writes into tbard's data folders.

use crate::{
    audible::{
        auth::{Auth, CustomerInfo, DeviceInfo},
        client::Client,
        library::LibraryItem,
    },
    config::Paths,
    db::Db,
    download::{ChapterFile, INDEX},
};
use anyhow::{bail, Result};
use image::{Rgb, RgbImage};
use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
use std::process::Command;

pub fn active() -> bool {
    cfg!(feature = "demo") && std::env::var_os("TBARD_DEMO").is_some()
}

/// (title, author, series, hours, percent listened)
const BOOKS: [(&str, &str, &str, f64, f64); 18] = [
    ("The Time Machine", "H. G. Wells", "", 3.6, 42.0),
    ("Frankenstein", "Mary Shelley", "", 8.6, 100.0),
    ("Dracula", "Bram Stoker", "", 15.5, 18.0),
    ("Pride and Prejudice", "Jane Austen", "", 11.9, 0.0),
    ("Moby-Dick", "Herman Melville", "", 24.8, 7.0),
    ("The War of the Worlds", "H. G. Wells", "", 6.8, 0.0),
    ("Alice's Adventures in Wonderland", "Lewis Carroll", "Alice 1", 2.6, 100.0),
    ("Through the Looking-Glass", "Lewis Carroll", "Alice 2", 3.0, 0.0),
    ("The Adventures of Sherlock Holmes", "Arthur Conan Doyle", "Sherlock Holmes 3", 11.4, 64.0),
    ("Twenty Thousand Leagues Under the Seas", "Jules Verne", "", 14.1, 0.0),
    ("Around the World in Eighty Days", "Jules Verne", "", 7.0, 100.0),
    ("Don Quixote", "Miguel de Cervantes", "", 38.9, 3.0),
    ("The Odyssey", "Homer", "", 12.2, 0.0),
    ("Treasure Island", "Robert Louis Stevenson", "", 6.9, 0.0),
    ("Jane Eyre", "Charlotte Brontë", "", 18.9, 0.0),
    ("The Count of Monte Cristo", "Alexandre Dumas", "", 52.6, 26.0),
    ("Great Expectations", "Charles Dickens", "", 18.4, 0.0),
    ("Les Misérables", "Victor Hugo", "", 60.2, 0.0),
];

/// The downloaded book, with this many chapters of silence.
const DOWNLOADED: usize = 0;
const CHAPTERS: [(&str, i64); 12] = [
    ("Opening Credits", 20),
    ("Chapter 1: Introduction", 960),
    ("Chapter 2: The Machine", 840),
    ("Chapter 3: The Time Traveller Returns", 1080),
    ("Chapter 4: Time Travelling", 900),
    ("Chapter 5: In the Golden Age", 1140),
    ("Chapter 6: The Sunset of Mankind", 780),
    ("Chapter 7: A Sudden Shock", 1020),
    ("Chapter 8: Explanation", 1260),
    ("Chapter 9: The Morlocks", 1200),
    ("Chapter 10: When Night Came", 1110),
    ("Epilogue", 300),
];

/// Fills the database, covers and one downloaded book; returns an offline client.
pub fn setup(paths: &Paths, db: &Db) -> Result<Client> {
    if db.is_empty()? {
        let items: Vec<LibraryItem> = BOOKS
            .iter()
            .enumerate()
            .map(|(i, (title, author, series, hours, percent))| LibraryItem {
                asin: asin(i),
                title: (*title).into(),
                authors: (*author).into(),
                narrators: "LibriVox".into(),
                series: (*series).into(),
                runtime_ms: (hours * 3_600_000.0) as i64,
                percent_complete: *percent,
                cover_url: None,
            })
            .collect();
        db.upsert_books(&items)?;
        for (i, (_, _, _, hours, percent)) in BOOKS.iter().enumerate() {
            cover(i).save(paths.cache.join("covers").join(format!("{}.jpg", asin(i))))?;
            if *percent > 0.0 && *percent < 100.0 {
                let pos = (hours * 3_600_000.0 * percent / 100.0) as i64;
                db.mark_synced(&asin(i), pos, crate::sync::now_ms() - i as i64 * 3_600_000)?;
            }
        }
        download(paths, db)?;
    }
    let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 1024)?;
    Client::from_auth(Auth {
        access_token: String::new(),
        refresh_token: String::new(),
        adp_token: String::new(),
        device_private_key: key.to_pkcs1_pem(LineEnding::LF)?.to_string(),
        expires: 0.0,
        locale_code: "us".into(),
        device_info: DeviceInfo {
            device_serial_number: String::new(),
            device_type: String::new(),
            device_name: String::new(),
        },
        customer_info: CustomerInfo { user_id: String::new(), name: "Demo".into() },
    })
}

fn asin(i: usize) -> String {
    format!("DEMO{i:06}")
}

/// Silent chapter files for the first book, so the player runs with real mpv.
fn download(paths: &Paths, db: &Db) -> Result<()> {
    let (title, author, ..) = BOOKS[DOWNLOADED];
    let dir = crate::download::book_dir(&paths.library, author, title);
    std::fs::create_dir_all(&dir)?;
    let ffmpeg = paths.ffmpeg()?;
    let (mut files, mut start) = (Vec::new(), 0);
    for (i, (name, secs)) in CHAPTERS.iter().enumerate() {
        let file = format!("{:03} - {name}.m4b", i + 1);
        let ok = Command::new(ffmpeg)
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=22050:cl=mono", "-t"])
            .arg(secs.to_string())
            .args(["-c:a", "aac", "-b:a", "16k", "-f", "mp4"])
            .arg(dir.join(&file))
            .status()?;
        if !ok.success() {
            bail!("ffmpeg could not create the demo audio");
        }
        files.push(ChapterFile { file, title: (*name).into(), start_ms: start, length_ms: secs * 1000 });
        start += secs * 1000;
    }
    std::fs::write(dir.join(INDEX), serde_json::to_vec_pretty(&files)?)?;
    cover(DOWNLOADED).save(dir.join("cover.jpg"))?;
    db.set_downloaded(&asin(DOWNLOADED), &dir, "demo", "AAX_44_128")
}

/// A generated 500×500 cover: concentric bands and a sun on a background,
/// colors taken from the book's index.
fn cover(i: usize) -> RgbImage {
    const PALETTE: [[u8; 3]; 8] = [
        [0xff, 0xb0, 0x00],
        [0x2e, 0x86, 0xab],
        [0xc7, 0x3e, 0x1d],
        [0x3b, 0x8e, 0x5a],
        [0x7d, 0x4e, 0xa8],
        [0xe8, 0xd5, 0xb5],
        [0x1f, 0x2a, 0x44],
        [0xd6, 0x6b, 0x8e],
    ];
    let (bg, band, sun) = (PALETTE[i % 8], PALETTE[(i + 3) % 8], PALETTE[(i + 5) % 8]);
    let (cx, cy) = (250.0 + (i % 3) as f64 * 40.0 - 40.0, 200.0 + (i % 4) as f64 * 25.0);
    RgbImage::from_fn(500, 500, |x, y| {
        let (dx, dy) = (x as f64 - cx, y as f64 - cy);
        let r = (dx * dx + dy * dy).sqrt();
        let c = if r < 70.0 {
            sun
        } else if y > 330 && (y / 18) % 2 == (i as u32 % 2) {
            band
        } else if (r as u32 / 30).is_multiple_of(2) && y < 330 {
            [bg[0].saturating_sub(25), bg[1].saturating_sub(25), bg[2].saturating_sub(25)]
        } else {
            bg
        };
        Rgb(c)
    })
}
