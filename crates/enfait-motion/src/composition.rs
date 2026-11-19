//! The contract of a composition and its preparation before rendering.
//!
//! A composition is a standalone HTML document declaring
//! `window.enfait = { duration, seek(t) }`. The engine opens it, waits for
//! fonts and images, then calls `seek(t)` for each frame before capturing it.

use crate::library;

/// Text of the contract, to give as is to the models.
pub const CONTRACT: &str = include_str!("contract.md");

/// Extracts the HTML document from a model answer: the last
/// ```` ```html ```` block if there is one, else the text between
/// `<!doctype` (or `<html`) and `</html>`.
pub fn extract_html(reply: &str) -> Option<String> {
    let fenced = last_fenced(reply, "html");
    let candidate = fenced.as_deref().unwrap_or(reply);
    let lower = candidate.to_ascii_lowercase();
    let start = lower.find("<!doctype").or_else(|| lower.find("<html"))?;
    let end = lower.rfind("</html>")?.checked_add("</html>".len())?;
    candidate.get(start..end).map(str::to_owned)
}

/// Content of the last ```` ```lang ```` code block.
pub fn last_fenced(text: &str, lang: &str) -> Option<String> {
    let open = format!("```{lang}");
    let start = text.rfind(&open)?;
    let body = text.get(start.checked_add(open.len())?..)?;
    let body = body.get(body.find('\n')?.checked_add(1)?..)?;
    let end = body.find("\n```").unwrap_or(body.len());
    body.get(..end).map(str::to_owned)
}

/// Adds the library (fonts, anime.js) at the top of the document.
pub fn prepare(html: &str) -> String {
    let tags = library::head_tags();
    let lower = html.to_ascii_lowercase();
    // `<head>` or `<head …>`, but not `<header>`.
    let head = lower
        .match_indices("<head")
        .find(|(i, _)| {
            lower
                .get(i.saturating_add(5)..)
                .is_some_and(|rest| rest.starts_with(['>', ' ', '\n', '\t']))
        })
        .map(|(i, _)| i);
    if let Some(pos) = head
        && let Some(close) = lower.get(pos..).and_then(|s| s.find('>'))
    {
        let at = pos.saturating_add(close).saturating_add(1);
        if let (Some(before), Some(after)) = (html.get(..at), html.get(at..)) {
            return format!("{before}\n{tags}{after}");
        }
    }
    format!("<!doctype html><html><head>{tags}</head><body>{html}</body></html>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_html() {
        let reply = "Voici :\n```html\n<!DOCTYPE html>\n<html><body>a</body></html>\n```\nFin.";
        assert_eq!(
            extract_html(reply).as_deref(),
            Some("<!DOCTYPE html>\n<html><body>a</body></html>")
        );
    }

    #[test]
    fn extracts_raw_html_and_takes_last_fence() {
        assert_eq!(
            extract_html("x <html>b</html> y").as_deref(),
            Some("<html>b</html>")
        );
        let two = "```html\n<html>1</html>\n```\n```html\n<html>2</html>\n```";
        assert_eq!(extract_html(two).as_deref(), Some("<html>2</html>"));
        assert_eq!(extract_html("rien"), None);
    }

    #[test]
    fn prepare_injects_after_head() {
        let out = prepare("<html><head><title>t</title></head><body></body></html>");
        let head = out.find("<head>").unwrap();
        let css = out.find("lib/fonts.css").unwrap();
        let title = out.find("<title>").unwrap();
        assert!(head < css && css < title);
    }
}
