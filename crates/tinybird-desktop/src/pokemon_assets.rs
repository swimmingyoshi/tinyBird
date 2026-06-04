use crate::game_addons::{AddonData, StreamSnapshot};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const SPRITE_CACHE_DIR: &str = "stream-data/pokemon-sprites";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(12);
const FAILED_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const SPRITE_MAX_DIMENSION: u32 = 96;

#[derive(Clone, Debug)]
pub struct SpriteBitmap {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

enum FetchResult {
    Loaded {
        species_id: u16,
        bitmap: SpriteBitmap,
    },
    Failed {
        species_id: u16,
    },
}

pub struct PokemonSpriteStore {
    loaded: HashMap<u16, SpriteBitmap>,
    pending: HashSet<u16>,
    failed_at: HashMap<u16, Instant>,
    sender: Sender<FetchResult>,
    receiver: Receiver<FetchResult>,
}

impl Default for PokemonSpriteStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PokemonSpriteStore {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            loaded: HashMap::new(),
            pending: HashSet::new(),
            failed_at: HashMap::new(),
            sender,
            receiver,
        }
    }

    pub fn queue_snapshot(&mut self, snapshot: &StreamSnapshot) {
        for species_id in species_ids_from_snapshot(snapshot) {
            self.ensure_species(species_id);
        }
    }

    pub fn drain_updates(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.receiver.try_recv() {
            match update {
                FetchResult::Loaded { species_id, bitmap } => {
                    self.pending.remove(&species_id);
                    self.failed_at.remove(&species_id);
                    self.loaded.insert(species_id, bitmap);
                    changed = true;
                }
                FetchResult::Failed { species_id } => {
                    self.pending.remove(&species_id);
                    self.failed_at.insert(species_id, Instant::now());
                }
            }
        }
        changed
    }

    pub fn sprite(&self, species_id: u16) -> Option<&SpriteBitmap> {
        self.loaded.get(&species_id)
    }

    fn ensure_species(&mut self, species_id: u16) {
        if self.loaded.contains_key(&species_id) || self.pending.contains(&species_id) {
            return;
        }

        if self
            .failed_at
            .get(&species_id)
            .is_some_and(|time| time.elapsed() < FAILED_RETRY_INTERVAL)
        {
            return;
        }

        if let Some(bitmap) = load_cached_sprite(species_id) {
            self.loaded.insert(species_id, bitmap);
            self.failed_at.remove(&species_id);
            return;
        }

        self.pending.insert(species_id);
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = fetch_sprite(species_id)
                .map(|(bitmap, bytes)| {
                    let _ = write_cached_sprite(species_id, &bytes);
                    FetchResult::Loaded { species_id, bitmap }
                })
                .unwrap_or(FetchResult::Failed { species_id });
            let _ = sender.send(result);
        });
    }
}

fn species_ids_from_snapshot(snapshot: &StreamSnapshot) -> Vec<u16> {
    let Some(addon) = snapshot.addon.as_ref() else {
        return Vec::new();
    };

    match &addon.data {
        AddonData::FireRed(data) => {
            let mut seen = HashSet::new();
            let mut ordered = Vec::with_capacity(data.party.len());
            for member in &data.party {
                if seen.insert(member.species_id) {
                    ordered.push(member.species_id);
                }
            }
            ordered
        }
    }
}

fn load_cached_sprite(species_id: u16) -> Option<SpriteBitmap> {
    let bytes = fs::read(sprite_cache_path(species_id)).ok()?;
    decode_sprite(&bytes).ok()
}

fn write_cached_sprite(species_id: u16, bytes: &[u8]) -> std::io::Result<()> {
    let path = sprite_cache_path(species_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

fn fetch_sprite(species_id: u16) -> Result<(SpriteBitmap, Vec<u8>), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent("tinyBird/0.1 (+https://github.com/OpenAI)")
        .build()
        .map_err(|err| format!("build client: {err}"))?;

    let response = client
        .get(sprite_url(species_id))
        .send()
        .map_err(|err| format!("download sprite: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("sprite response {}", response.status()));
    }

    let bytes = response
        .bytes()
        .map_err(|err| format!("read sprite bytes: {err}"))?
        .to_vec();
    let bitmap = decode_sprite(&bytes).map_err(|err| format!("decode sprite: {err}"))?;
    Ok((bitmap, bytes))
}

fn decode_sprite(bytes: &[u8]) -> Result<SpriteBitmap, image::ImageError> {
    let image = image::load_from_memory(bytes)?.into_rgba8();
    let image = image::imageops::thumbnail(&image, SPRITE_MAX_DIMENSION, SPRITE_MAX_DIMENSION);
    let (width, height) = image.dimensions();
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        pixels.push(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
    }

    Ok(SpriteBitmap {
        width: width as usize,
        height: height as usize,
        pixels,
    })
}

fn sprite_url(species_id: u16) -> String {
    format!(
        "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/{species_id}.png"
    )
}

fn sprite_cache_path(species_id: u16) -> PathBuf {
    Path::new(SPRITE_CACHE_DIR).join(format!("{species_id}.png"))
}
