//! Every page's header links, in the same order.
//!
//! The header once moved depending on which page you were standing on, because
//! a link added later was appended by hand to each copy in a different place:
//! exactly the kind of thing nobody notices while writing a page and everybody
//! notices while using it.
//!
//! The markup stays hand-written per page, because a nav rendered by script is
//! a nav that is missing before the script runs. This test is what keeps the
//! copies agreeing.

use std::path::Path;

/// The order, once. Every page carries all of them.
const ORDER: [(&str, &str); 3] = [("/", "Start"), ("/play", "Play"), ("/addons", "Add-ons")];

const PAGES: [&str; 3] = ["index.html", "play.html", "addons.html"];

/// The `href`/text pairs inside the page's `bar__nav`, in document order.
fn nav_links(html: &str) -> Vec<(String, String)> {
    let start = html
        .find("<nav class=\"bar__nav\"")
        .expect("page has a bar__nav");
    let end = html[start..].find("</nav>").expect("bar__nav is closed") + start;
    let nav = &html[start..end];

    let mut links = Vec::new();
    let mut rest = nav;
    while let Some(anchor) = rest.find("<a ") {
        rest = &rest[anchor..];
        let Some(href_at) = rest.find("href=\"") else {
            break;
        };
        let after = &rest[href_at + 6..];
        let href = &after[..after.find('"').expect("href is closed")];
        let Some(text_at) = rest.find('>') else { break };
        let text = &rest[text_at + 1..];
        let text = &text[..text.find('<').expect("anchor is closed")];
        links.push((href.to_string(), text.trim().to_string()));
        rest = &rest[text_at + 1..];
    }
    links
}

fn page(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/assets")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {name}: {err}"))
}

#[test]
fn every_page_lists_the_header_links_in_the_same_order() {
    for name in PAGES {
        let html = page(name);
        let links = nav_links(&html);

        let expected: Vec<(String, String)> = ORDER
            .iter()
            .map(|(href, text)| (href.to_string(), text.to_string()))
            .collect();

        assert_eq!(
            links, expected,
            "{name} lists its header links in a different order from the rest of the site"
        );
    }
}

#[test]
fn every_page_marks_itself_as_the_current_one() {
    for (name, current) in PAGES.iter().zip(["/", "/play", "/addons"]) {
        let html = page(name);
        let start = html.find("<nav class=\"bar__nav\"").expect("bar__nav");
        let end = html[start..].find("</nav>").expect("closed") + start;
        let nav = &html[start..end];

        // Exactly one link is the current page, and it is this page's own.
        let marked: Vec<&str> = nav
            .split("<a ")
            .skip(1)
            .filter(|anchor| anchor.contains("aria-current=\"page\""))
            .map(|anchor| {
                let at = anchor.find("href=\"").expect("href") + 6;
                &anchor[at..at + anchor[at..].find('"').expect("closed")]
            })
            .collect();
        assert_eq!(marked, vec![current], "{name} marks the wrong current page");
    }
}

/// Every link in the nav has to go somewhere the server actually serves.
#[test]
fn the_header_never_links_somewhere_that_is_not_routed() {
    let routes = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
        .expect("read main.rs");

    for name in PAGES {
        for (href, text) in nav_links(&page(name)) {
            let routed = href == "/" || routes.contains(&format!("\"{href}\""));
            assert!(
                routed,
                "{name} links to {text} at {href}, which main.rs does not route"
            );
        }
    }
}
