//! Region capture, pattern search, and state diffing.
//!
//! Regions are snapshotted into flat buffers up front rather than being read
//! through the bus during the search: a full EWRAM scan is 256 KB of reads, and
//! doing that once per pattern instead of once per byte comparison keeps a
//! multi-pattern run instant.

/// A captured copy of one memory region.
pub struct MemoryRegion {
    pub name: &'static str,
    pub base: u32,
    pub bytes: Vec<u8>,
}

impl MemoryRegion {
    fn contains(&self, address: u32) -> bool {
        address >= self.base && ((address - self.base) as usize) < self.bytes.len()
    }
}

/// Which regions to search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regions {
    Ewram,
    Iwram,
    All,
}

impl Regions {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.to_ascii_lowercase().as_str() {
            "ewram" | "wram" | "e" => Ok(Regions::Ewram),
            "iwram" | "i" => Ok(Regions::Iwram),
            "all" | "both" => Ok(Regions::All),
            other => Err(format!("unknown region '{other}' (try ewram, iwram, all)")),
        }
    }

    /// `(name, base, length)` for each region this selection covers.
    pub fn list(self) -> Vec<(&'static str, u32, u32)> {
        const EWRAM: (&str, u32, u32) = ("ewram", 0x0200_0000, 0x0004_0000);
        const IWRAM: (&str, u32, u32) = ("iwram", 0x0300_0000, 0x0000_8000);
        match self {
            Regions::Ewram => vec![EWRAM],
            Regions::Iwram => vec![IWRAM],
            Regions::All => vec![EWRAM, IWRAM],
        }
    }
}

/// One match of a search pattern.
pub struct Hit {
    pub address: u32,
    pub region: &'static str,
}

/// Every occurrence of `pattern` across `regions`, in address order.
pub fn find(regions: &[MemoryRegion], pattern: &[u8]) -> Vec<Hit> {
    if pattern.is_empty() {
        return Vec::new();
    }

    let mut hits = Vec::new();
    for region in regions {
        if region.bytes.len() < pattern.len() {
            continue;
        }
        for (offset, window) in region.bytes.windows(pattern.len()).enumerate() {
            if window == pattern {
                hits.push(Hit {
                    address: region.base + offset as u32,
                    region: region.name,
                });
            }
        }
    }
    hits
}

/// Read `len` bytes starting at `address`, zero-filling anything outside the
/// captured regions so a dump of a bad address is obvious rather than fatal.
pub fn read(regions: &[MemoryRegion], address: u32, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    for (index, slot) in out.iter_mut().enumerate() {
        let target = address.wrapping_add(index as u32);
        if let Some(region) = regions.iter().find(|region| region.contains(target)) {
            *slot = region.bytes[(target - region.base) as usize];
        }
    }
    out
}

/// Find text whose encoding offset is unknown.
///
/// Matches any byte run whose consecutive differences equal those of `text`.
/// That holds for every encoding where the alphabet is one contiguous ascending
/// block, which covers nearly all GBA games — including the ones that shift the
/// whole table so a plain ASCII search finds nothing.
///
/// Use a single-case word: mixed case only works if the gap between `Z` and `a`
/// happens to match ASCII, which it usually does not.
pub fn find_relative(regions: &[MemoryRegion], text: &str) -> Vec<RelativeHit> {
    // Games store text one byte per character, but also as 16-bit codes, and
    // sometimes one character per field of a wider record. Trying a few strides
    // costs little and is the difference between finding the name and
    // concluding, wrongly, that it is not in RAM.
    const STRIDES: [usize; 3] = [1, 2, 4];

    let chars: Vec<u8> = text.bytes().collect();
    if chars.len() < 2 {
        return Vec::new();
    }
    let deltas: Vec<u8> = chars
        .windows(2)
        .map(|pair| pair[1].wrapping_sub(pair[0]))
        .collect();

    let mut hits = Vec::new();
    for &stride in &STRIDES {
        let span = deltas.len() * stride;
        for region in regions {
            if region.bytes.len() <= span {
                continue;
            }
            for start in 0..region.bytes.len() - span {
                let matches = deltas.iter().enumerate().all(|(index, delta)| {
                    let here = region.bytes[start + index * stride];
                    let next = region.bytes[start + (index + 1) * stride];
                    next.wrapping_sub(here) == *delta
                });
                if matches {
                    hits.push(RelativeHit {
                        address: region.base + start as u32,
                        region: region.name,
                        stride,
                        first_byte: region.bytes[start],
                        implied_offset: region.bytes[start].wrapping_sub(chars[0]),
                    });
                }
            }
        }
    }
    hits
}

/// A match from [`find_relative`], carrying the encoding offset it implies.
pub struct RelativeHit {
    pub address: u32,
    pub region: &'static str,
    /// Bytes between consecutive characters: 1 for plain text, 2 for 16-bit.
    pub stride: usize,
    pub first_byte: u8,
    pub implied_offset: u8,
}

/// A contiguous stretch of bytes that differs between two captures.
pub struct DiffRun {
    pub address: u32,
    pub len: usize,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

/// Group differing bytes into runs.
///
/// Runs, not individual bytes, because a changed 32-bit counter shows up as up
/// to four adjacent differences and is far easier to recognise as one entry.
pub fn diff_runs(before: &[MemoryRegion], after: &[MemoryRegion]) -> Vec<DiffRun> {
    let mut runs = Vec::new();

    for old in before {
        let Some(new) = after.iter().find(|region| region.name == old.name) else {
            continue;
        };
        let len = old.bytes.len().min(new.bytes.len());

        let mut index = 0;
        while index < len {
            if old.bytes[index] == new.bytes[index] {
                index += 1;
                continue;
            }
            let start = index;
            while index < len && old.bytes[index] != new.bytes[index] {
                index += 1;
            }
            runs.push(DiffRun {
                address: old.base + start as u32,
                len: index - start,
                before: old.bytes[start..index].to_vec(),
                after: new.bytes[start..index].to_vec(),
            });
        }
    }

    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(name: &'static str, base: u32, bytes: Vec<u8>) -> MemoryRegion {
        MemoryRegion { name, base, bytes }
    }

    #[test]
    fn find_reports_absolute_addresses() {
        let regions = vec![region("ewram", 0x0200_0000, vec![0, 1, 2, 3, 4, 1, 2])];
        let hits = find(&regions, &[1, 2]);

        let addresses: Vec<_> = hits.iter().map(|hit| hit.address).collect();
        assert_eq!(addresses, vec![0x0200_0001, 0x0200_0005]);
        assert_eq!(hits[0].region, "ewram");
    }

    #[test]
    fn find_searches_every_selected_region() {
        let regions = vec![
            region("ewram", 0x0200_0000, vec![9, 9]),
            region("iwram", 0x0300_0000, vec![0, 9, 9]),
        ];
        let hits = find(&regions, &[9, 9]);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].address, 0x0300_0001);
    }

    #[test]
    fn find_handles_patterns_longer_than_the_region() {
        let regions = vec![region("ewram", 0x0200_0000, vec![1, 2])];
        assert!(find(&regions, &[1, 2, 3, 4]).is_empty());
        assert!(find(&regions, &[]).is_empty());
    }

    #[test]
    fn read_zero_fills_outside_the_capture() {
        let regions = vec![region("ewram", 0x0200_0000, vec![0xAA, 0xBB])];
        assert_eq!(read(&regions, 0x0200_0000, 4), vec![0xAA, 0xBB, 0, 0]);
        assert_eq!(read(&regions, 0x0500_0000, 2), vec![0, 0]);
    }

    #[test]
    fn diff_groups_adjacent_changes_into_one_run() {
        // A changed 32-bit counter must read as one entry, not four.
        let before = vec![region("ewram", 0x0200_0000, vec![0, 0, 0, 0, 5, 0])];
        let after = vec![region("ewram", 0x0200_0000, vec![1, 2, 3, 4, 5, 0])];

        let runs = diff_runs(&before, &after);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].address, 0x0200_0000);
        assert_eq!(runs[0].len, 4);
        assert_eq!(runs[0].after, vec![1, 2, 3, 4]);
    }

    #[test]
    fn diff_separates_runs_split_by_matching_bytes() {
        let before = vec![region("ewram", 0x0200_0000, vec![0, 0, 0, 0])];
        let after = vec![region("ewram", 0x0200_0000, vec![1, 0, 0, 1])];

        let runs = diff_runs(&before, &after);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].address, 0x0200_0000);
        assert_eq!(runs[1].address, 0x0200_0003);
    }

    #[test]
    fn identical_captures_produce_no_runs() {
        let capture = vec![region("ewram", 0x0200_0000, vec![1, 2, 3])];
        let same = vec![region("ewram", 0x0200_0000, vec![1, 2, 3])];
        assert!(diff_runs(&capture, &same).is_empty());
    }

    #[test]
    fn relative_search_finds_text_under_an_unknown_offset() {
        // "arche" shifted by +0x6A, which is how FFTA stores lowercase letters.
        let shifted: Vec<u8> = "arche".bytes().map(|b| b.wrapping_add(0x6A)).collect();
        let mut bytes = vec![0u8; 4];
        bytes.extend(&shifted);
        let regions = vec![region("ewram", 0x0200_0000, bytes)];

        let hits = find_relative(&regions, "arche");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].address, 0x0200_0004);
        assert_eq!(hits[0].implied_offset, 0x6A);
        assert_eq!(hits[0].stride, 1);
    }

    #[test]
    fn relative_search_finds_plain_ascii_too() {
        let regions = vec![region("ewram", 0x0200_0000, b"xxarchexx".to_vec())];
        let hits = find_relative(&regions, "arche");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].implied_offset, 0);
    }

    #[test]
    fn relative_search_finds_sixteen_bit_text() {
        // Each character in the low byte of a 16-bit code, high byte unrelated.
        let mut bytes = vec![0u8; 4];
        for ch in "arche".bytes() {
            bytes.push(ch.wrapping_add(0x20));
            bytes.push(0x40);
        }
        let regions = vec![region("ewram", 0x0200_0000, bytes)];

        let hits = find_relative(&regions, "arche");
        let wide = hits.iter().find(|hit| hit.stride == 2).expect("stride 2 hit");
        assert_eq!(wide.address, 0x0200_0004);
        assert_eq!(wide.implied_offset, 0x20);
    }

    #[test]
    fn relative_search_needs_at_least_two_characters() {
        let regions = vec![region("ewram", 0x0200_0000, vec![1, 2, 3])];
        assert!(find_relative(&regions, "a").is_empty());
        assert!(find_relative(&regions, "").is_empty());
    }

    #[test]
    fn region_selection_parses_the_documented_names() {
        assert_eq!(Regions::parse("ewram").unwrap(), Regions::Ewram);
        assert_eq!(Regions::parse("IWRAM").unwrap(), Regions::Iwram);
        assert_eq!(Regions::parse("all").unwrap(), Regions::All);
        assert!(Regions::parse("vram").is_err());
    }

    #[test]
    fn region_extents_match_the_gba_memory_map() {
        let all = Regions::All.list();
        assert_eq!(all[0], ("ewram", 0x0200_0000, 0x0004_0000));
        assert_eq!(all[1], ("iwram", 0x0300_0000, 0x0000_8000));
    }
}
