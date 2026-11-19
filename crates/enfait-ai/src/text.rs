//! Small tools on the text of the replies.

/// Content of the last ```` ```lang ```` code block.
pub fn last_fenced<'a>(text: &'a str, lang: &str) -> Option<&'a str> {
    let open = format!("```{lang}");
    let start = text.rfind(&open)?;
    let body = text.get(start.checked_add(open.len())?..)?;
    let body = body.get(body.find('\n')?.checked_add(1)?..)?;
    let end = body.find("\n```").unwrap_or(body.len());
    body.get(..end)
}

/// Replaces the `{{KEYS}}` of a prompt template.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values
        .iter()
        .fold(template.to_owned(), |acc, (key, value)| {
            acc.replace(&format!("{{{{{key}}}}}"), value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_and_fill() {
        assert_eq!(last_fenced("a\n```json\n{}\n```", "json"), Some("{}"));
        assert_eq!(fill("x {{A}} {{A}}", &[("A", "1")]), "x 1 1");
    }
}
