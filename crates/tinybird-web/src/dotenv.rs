//! A minimal `.env` loader.
//!
//! Secrets belong in a gitignored `.env`, not in source and not in the repo's
//! history. This is small enough not to warrant a dependency, and keeping it
//! here means `git clone && cp .env.example .env && cargo run` is the whole
//! setup story.
//!
//! Real environment variables always win, so CI and container deployments can
//! override the file without editing it.

use std::collections::HashMap;
use std::env;
use std::path::Path;

/// Read `path` and set any variable that is not already present in the
/// environment. Missing files are not an error — the app must run without one.
pub fn load(path: impl AsRef<Path>) {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };
    for (key, value) in parse(&contents) {
        if env::var_os(&key).is_none() {
            // Safe here: called once from `main` before any threads are spawned.
            env::set_var(key, value);
        }
    }
}

/// Parse `.env` text into key/value pairs.
///
/// Supports `KEY=value`, `export KEY=value`, `#` comments, blank lines, and
/// single- or double-quoted values. Anything else is skipped rather than
/// rejected: a malformed line should not stop the server from starting.
pub fn parse(contents: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        vars.insert(key.to_string(), unquote(value.trim()));
    }

    vars
}

/// Strip a matching pair of surrounding quotes.
///
/// Only a matching pair: `"abc` is a value that happens to start with a quote,
/// not a broken string, and guessing would corrupt a key that contains one.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return value[1..value.len() - 1].to_string();
        }
    }
    // An unquoted value ends at the first inline comment.
    match value.split_once(" #") {
        Some((head, _)) => head.trim_end().to_string(),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_assignments() {
        let vars = parse("TINYBIRD_MEDIA_KEY=abc123\nTINYBIRD_MEDIA_VAULT=tinybird\n");
        assert_eq!(vars["TINYBIRD_MEDIA_KEY"], "abc123");
        assert_eq!(vars["TINYBIRD_MEDIA_VAULT"], "tinybird");
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let vars = parse("# a comment\n\n  \nKEY=value\n");
        assert_eq!(vars.len(), 1);
        assert_eq!(vars["KEY"], "value");
    }

    #[test]
    fn accepts_the_export_prefix() {
        let vars = parse("export KEY=value");
        assert_eq!(vars["KEY"], "value");
    }

    #[test]
    fn strips_matching_quotes() {
        let vars = parse("A=\"quoted\"\nB='single'\n");
        assert_eq!(vars["A"], "quoted");
        assert_eq!(vars["B"], "single");
    }

    #[test]
    fn leaves_unmatched_quotes_alone() {
        // A key could legitimately contain a quote; guessing would corrupt it.
        let vars = parse("A=\"unclosed\nB=trailing\"");
        assert_eq!(vars["A"], "\"unclosed");
        assert_eq!(vars["B"], "trailing\"");
    }

    #[test]
    fn keeps_equals_signs_inside_values() {
        // Base64 and JWT-shaped keys routinely end in padding.
        let vars = parse("KEY=abc==");
        assert_eq!(vars["KEY"], "abc==");
    }

    #[test]
    fn trims_whitespace_around_the_key_and_value() {
        let vars = parse("  KEY  =  value  ");
        assert_eq!(vars["KEY"], "value");
    }

    #[test]
    fn drops_inline_comments_from_unquoted_values() {
        let vars = parse("KEY=value # trailing note");
        assert_eq!(vars["KEY"], "value");
    }

    #[test]
    fn keeps_hashes_inside_quoted_values() {
        // A generated key can contain a '#'; quoting is how you protect it.
        let vars = parse("KEY=\"va#lue\"");
        assert_eq!(vars["KEY"], "va#lue");
    }

    #[test]
    fn skips_lines_without_an_assignment() {
        let vars = parse("JUST_A_WORD\nKEY=value");
        assert_eq!(vars.len(), 1);
        assert!(vars.contains_key("KEY"));
    }

    #[test]
    fn skips_an_empty_key() {
        assert!(parse("=orphaned").is_empty());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        load("definitely/not/here/.env");
    }
}
