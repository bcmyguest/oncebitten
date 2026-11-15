//! Attachment links in a pull request's text, as evidence for a diff rule.
//! `gate pr` reads the description and comments of the PR and asks whether an
//! attachment link satisfies a rule's `require_changed` globs.

use std::sync::LazyLock;

use regex::Regex;

/// Extensions a GitHub `user-attachments/assets/<uuid>` URL can stand for.
/// The URL has no extension; inside an image tag it is an image.
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "svg"];
const USER_ATTACHMENTS: &str = "/user-attachments/assets/";

/// `![alt](url)` and `[text](url)`; group 1 is `!` for an image, group 2 the URL.
static MARKDOWN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(!?)\[[^\]]*\]\(\s*<?([^\s)>]+)>?(?:\s+"[^"]*")?\s*\)"#).unwrap()
});
/// `<img ... src="url" ...>` with either quote style; group 1 or 2 is the URL.
static HTML_IMG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<img\b[^>]*?\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap()
});
static BARE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>"'()\[\]]+"#).unwrap());

/// How a link appeared in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Image,
    Link,
}

/// Whether `text` carries an attachment link whose extension matches one of
/// the `require_changed` globs. A glob with no literal extension (such as
/// `docs/**`) matches no link.
pub fn satisfies(text: &str, require_changed: &[String]) -> bool {
    let wanted: Vec<String> = require_changed
        .iter()
        .flat_map(|glob| glob_extensions(glob))
        .collect();
    if wanted.is_empty() {
        return false;
    }
    links(text)
        .into_iter()
        .any(|(url, form)| match extension(&url) {
            Some(ext) => wanted.contains(&ext),
            None => {
                form == Form::Image
                    && url_path(&url).contains(USER_ATTACHMENTS)
                    && wanted
                        .iter()
                        .any(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
            }
        })
}

/// The literal extensions a glob ends in, lowercase: `*.png` gives `png`,
/// `*.{png,jpg}` gives both. Empty when the last segment has no literal one.
fn glob_extensions(glob: &str) -> Vec<String> {
    let name = glob.rsplit('/').next().unwrap_or(glob);
    let Some((_, ext)) = name.rsplit_once('.') else {
        return Vec::new();
    };
    let options: Vec<&str> = match ext.strip_prefix('{').and_then(|e| e.strip_suffix('}')) {
        Some(list) => list.split(',').collect(),
        None => vec![ext],
    };
    options
        .into_iter()
        .filter(|option| !option.is_empty() && !option.contains(['*', '?', '[', ']', '{', '}']))
        .map(str::to_lowercase)
        .collect()
}

/// Every link in `text` with the form it took. A URL inside markdown or an
/// `<img>` tag is not counted again as a bare URL.
fn links(text: &str) -> Vec<(String, Form)> {
    let mut out = Vec::new();
    let mut covered: Vec<(usize, usize)> = Vec::new();
    for caps in MARKDOWN.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        covered.push((whole.start(), whole.end()));
        let form = if &caps[1] == "!" {
            Form::Image
        } else {
            Form::Link
        };
        out.push((caps[2].to_owned(), form));
    }
    for caps in HTML_IMG.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        covered.push((whole.start(), whole.end()));
        let url = caps.get(1).or_else(|| caps.get(2)).unwrap().as_str();
        out.push((url.trim().to_owned(), Form::Image));
    }
    for found in BARE_URL.find_iter(text) {
        if covered
            .iter()
            .any(|&(start, end)| found.start() >= start && found.start() < end)
        {
            continue;
        }
        let url = found
            .as_str()
            .trim_end_matches(['.', ',', ';', ':', '!', '?']);
        out.push((url.to_owned(), Form::Link));
    }
    out
}

/// The URL without its query and fragment.
fn url_path(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// The lowercase extension of the URL's last path segment, if it has one.
fn extension(url: &str) -> Option<String> {
    let path = url_path(url);
    let name = path.rsplit('/').next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globs(patterns: &[&str]) -> Vec<String> {
        patterns.iter().map(|p| (*p).to_owned()).collect()
    }

    const PNG: &[&str] = &["docs/**/*.png"];
    const UUID_URL: &str =
        "https://github.com/user-attachments/assets/3f2a1c9e-5b7d-4e8a-9c1f-0a2b3c4d5e6f";

    #[test]
    fn a_markdown_image_matches_its_extension() {
        assert!(satisfies("![shot](https://x.test/a/shot.png)", &globs(PNG)));
        assert!(satisfies(
            "![shot](https://x.test/a/SHOT.PNG?raw=1)",
            &globs(PNG)
        ));
        assert!(!satisfies(
            "![shot](https://x.test/a/shot.jpg)",
            &globs(PNG)
        ));
    }

    #[test]
    fn a_markdown_link_with_a_title_matches() {
        assert!(satisfies(
            r#"see [the diagram](https://x.test/d/arch.svg "Architecture")"#,
            &globs(&["*.svg"])
        ));
    }

    #[test]
    fn a_bare_url_ending_in_the_extension_matches() {
        assert!(satisfies(
            "proof: https://x.test/a/run.gif",
            &globs(&["*.gif"])
        ));
        assert!(satisfies(
            "proof: https://x.test/a/run.gif.",
            &globs(&["*.gif"])
        ));
        assert!(!satisfies(
            "proof: https://x.test/a/run.gifs",
            &globs(&["*.gif"])
        ));
        assert!(!satisfies("see https://x.test/page", &globs(&["*.gif"])));
    }

    #[test]
    fn an_html_img_src_matches_with_either_quote() {
        assert!(satisfies(
            r#"<img width="300" src="https://x.test/a/b.webp" alt="x">"#,
            &globs(&["*.webp"])
        ));
        assert!(satisfies(
            "<img src='https://x.test/a/b.jpeg'>",
            &globs(&["*.jpeg"])
        ));
        assert!(satisfies(
            "<IMG\n  SRC=\"https://x.test/a/b.png\"\n/>",
            &globs(PNG)
        ));
    }

    #[test]
    fn a_user_attachments_url_counts_as_an_image_in_an_image_form_only() {
        let png = globs(PNG);
        assert!(satisfies(&format!("![img]({UUID_URL})"), &png));
        assert!(satisfies(
            &format!(r#"<img src="{UUID_URL}" alt="x">"#),
            &png
        ));
        assert!(!satisfies(&format!("[img]({UUID_URL})"), &png));
        assert!(!satisfies(UUID_URL, &png));
        // It stands for an image, so a non-image glob is not met.
        assert!(!satisfies(
            &format!("![img]({UUID_URL})"),
            &globs(&["*.pdf"])
        ));
    }

    #[test]
    fn a_glob_without_a_literal_extension_matches_nothing() {
        assert!(!satisfies(
            "![shot](https://x.test/a/shot.png)",
            &globs(&["docs/**"])
        ));
        assert!(!satisfies(&format!("![i]({UUID_URL})"), &globs(&["*.*"])));
        assert!(!satisfies("![shot](https://x.test/a/shot.png)", &[]));
    }

    #[test]
    fn a_brace_glob_offers_each_extension() {
        let glob = globs(&["shots/*.{png,jpg}"]);
        assert!(satisfies("![a](https://x.test/a.jpg)", &glob));
        assert!(satisfies("![a](https://x.test/a.png)", &glob));
        assert!(!satisfies("![a](https://x.test/a.gif)", &glob));
    }

    #[test]
    fn a_url_in_markdown_is_not_read_twice() {
        let links = links("![a](https://x.test/a.png) and https://x.test/b.png");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].1, Form::Image);
        assert_eq!(links[1].1, Form::Link);
    }
}
