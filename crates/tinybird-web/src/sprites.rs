//! Species pictures for the addon read-out.
//!
//! The party panel wants a picture next to every member, and the addon names
//! one as a path (`/sprites/32`) rather than a URL, so that deciding where
//! pictures actually come from is the host's problem and not the addon's.
//! This module is that decision for the web host.
//!
//! Sprites are fetched once and written to disk, so the first run of a server
//! populates itself and every run after it works with no network at all.
//!
//! The art is the Gen-3 era pixel sprite rather than the painted artwork the
//! desktop overlay uses. The read-out is monospace on an 8px grid with hard
//! edges everywhere; a soft full-colour illustration in that column would be
//! the only thing on the page pretending it is not made of pixels.
//!
//! That difference is also why the cache has its own subdirectory rather than
//! sharing the desktop's. Both fetch `{species_id}.png` from the same project,
//! but from different directories within it, and one cache keyed only by
//! species would serve whichever art style happened to be fetched first.

use std::path::{Path, PathBuf};
use std::time::Duration;

const SPRITE_SOURCE: &str =
    "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon";
/// Subdirectory of the sprite cache holding pixel art specifically.
///
/// The desktop app caches painted artwork as `{sprite_dir}/{id}.png`; writing
/// pixel sprites to those same names would have each frontend serve whatever
/// the other fetched first.
const PIXEL_CACHE_SUBDIR: &str = "pixel";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Generous, but bounded: this caps what one request can pull into memory.
const MAX_SPRITE_BYTES: usize = 512 * 1024;
/// Highest species number worth asking about.
///
/// The point is not to track the National Dex — it is that a species id comes
/// out of emulated memory, and a game in a strange state can report any `u16`.
/// Without a bound, a corrupt party turns into unbounded outbound requests.
const MAX_SPECIES_ID: u16 = 1025;

/// What a lookup produced, so the route can answer with the right status.
pub enum Sprite {
    /// Bytes to serve.
    Png(Vec<u8>),
    /// No such species; nothing was requested from anywhere.
    OutOfRange,
    /// Not on disk and could not be fetched. Offline is the usual reason.
    Unavailable,
}

/// Read a species sprite from the cache, fetching it once if it is not there.
pub async fn fetch(dir: &Path, species_id: u16) -> Sprite {
    if species_id == 0 || species_id > MAX_SPECIES_ID {
        return Sprite::OutOfRange;
    }

    let path = cache_path(dir, species_id);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if !bytes.is_empty() {
            return Sprite::Png(bytes);
        }
    }

    let Some(bytes) = download(species_id).await else {
        return Sprite::Unavailable;
    };

    // A cache write that fails is not worth failing the request over: the
    // picture is in hand, and the next request will simply fetch it again.
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&path, &bytes).await;

    Sprite::Png(bytes)
}

async fn download(species_id: u16) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .ok()?;

    let response = client
        .get(format!("{SPRITE_SOURCE}/{species_id}.png"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }

    // Length first where the server declares one, so an unexpectedly huge
    // response is refused before it is read rather than after.
    if response
        .content_length()
        .is_some_and(|len| len > MAX_SPRITE_BYTES as u64)
    {
        return None;
    }

    let bytes = response.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > MAX_SPRITE_BYTES {
        return None;
    }
    // Cheap sanity check: a PNG starts with a fixed eight-byte signature, and
    // caching an error page under a species number would poison that species
    // until someone cleared the directory by hand.
    if !bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return None;
    }

    Some(bytes.to_vec())
}

fn cache_path(dir: &Path, species_id: u16) -> PathBuf {
    dir.join(PIXEL_CACHE_SUBDIR)
        .join(format!("{species_id}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_species_id_out_of_range_never_reaches_the_network() {
        // Species ids come out of emulated memory. A game mid-load or a corrupt
        // save can report anything, and each bad id would otherwise be an
        // outbound request that is guaranteed to miss.
        let dir = Path::new("does-not-matter");
        for id in [0, MAX_SPECIES_ID + 1, u16::MAX] {
            assert!(
                matches!(fetch(dir, id).await, Sprite::OutOfRange),
                "species {id} should be refused before any lookup"
            );
        }
    }

    #[test]
    fn sprites_are_cached_by_species_so_a_duplicate_costs_nothing() {
        // Two of the same species in a party are one cache entry, not two.
        assert_eq!(cache_path(Path::new("cache"), 32), cache_path(Path::new("cache"), 32));
    }

    /// The desktop caches painted artwork under the bare species number in the
    /// same directory. Sharing those names would have each frontend serve
    /// whichever art style the other happened to fetch first.
    #[test]
    fn pixel_art_is_cached_apart_from_the_desktop_artwork() {
        let dir = Path::new("cache");
        let desktop_would_use = dir.join("32.png");

        assert_ne!(cache_path(dir, 32), desktop_would_use);
        assert!(cache_path(dir, 32).starts_with(dir.join(PIXEL_CACHE_SUBDIR)));
    }
}
