//! Redaction helpers so logs never contain credentials.

use url::Url;

/// Query parameter names whose values are always masked (compared case-insensitively).
const SENSITIVE_QUERY_KEYS: &[&str] = &[
    "key",
    "apikey",
    "api_key",
    "access_token",
    "refresh_token",
    "token",
    "auth",
    "authorization",
    "password",
    "passwd",
    "secret",
    "client_secret",
    "signature",
    "sig",
    "session",
    "sessionid",
    "steamloginsecure",
    "webapi_token",
];

/// Maximum number of characters kept for a single command-line argument.
pub const MAX_ARG_CHARS: usize = 160;

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SENSITIVE_QUERY_KEYS.contains(&lower.as_str())
        || lower.ends_with("_token")
        || lower.ends_with("_key")
        || lower.ends_with("password")
        || lower.ends_with("secret")
}

/// Returns the URL with user info removed and sensitive query values masked.
pub fn redact_url(url: &Url) -> String {
    let mut out = url.clone();
    if !out.username().is_empty() || out.password().is_some() {
        let _ = out.set_username("***");
        let _ = out.set_password(None);
    }
    if out.query().is_some() {
        let pairs: Vec<(String, String)> = out
            .query_pairs()
            .map(|(k, v)| {
                let masked = if is_sensitive_key(&k) {
                    "***".to_string()
                } else {
                    v.into_owned()
                };
                (k.into_owned(), masked)
            })
            .collect();
        if pairs.is_empty() {
            out.set_query(None);
        } else {
            out.query_pairs_mut().clear().extend_pairs(pairs);
        }
    }
    out.set_fragment(None);
    out.to_string()
}

/// Redacts a URL given as text; non-URL input is returned unchanged.
pub fn redact_url_str(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(url) if url.has_host() => redact_url(&url),
        _ => raw.to_string(),
    }
}

/// Truncates long text to `max_chars` characters, noting the original length.
pub fn truncate(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}...({count} chars total)")
}

/// Makes a single command-line argument safe and readable for logs.
pub fn redact_arg(arg: &str) -> String {
    let lower = arg.to_ascii_lowercase();
    if let Some((flag, _)) = arg.split_once('=') {
        let flag_key = flag.trim_start_matches('-');
        if flag.starts_with('-') && is_sensitive_key(&flag_key.replace('-', "_")) {
            return format!("{flag}=***");
        }
    }
    if lower.contains("://") {
        return truncate(&redact_url_str(arg), MAX_ARG_CHARS);
    }
    truncate(arg, MAX_ARG_CHARS)
}

/// Formats a program and its arguments as a single shell-like line for logs.
///
/// Values following sensitive flags such as `--token` are masked, URLs are redacted and long
/// arguments (inline scripts, JSON blobs) are truncated.
pub fn format_command_line<I, S>(program: &str, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = quote(program);
    let mut mask_next = false;
    for arg in args {
        let arg = arg.as_ref();
        out.push(' ');
        if mask_next {
            out.push_str("***");
            mask_next = false;
            continue;
        }
        if arg.starts_with('-') && !arg.contains('=') {
            let key = arg.trim_start_matches('-').replace('-', "_");
            mask_next = !key.is_empty() && is_sensitive_key(&key);
        }
        out.push_str(&quote(&redact_arg(arg)));
    }
    out
}

fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && !arg
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '$' | '`'))
    {
        return arg.to_string();
    }
    format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Formats an error together with its `source()` chain on one line.
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_sensitive_query_values_and_userinfo() {
        let url = Url::parse(
            "https://user:pw@api.steampowered.com/ISteamRemoteStorage/x/?key=ABC&itemcount=1&access_token=t#frag",
        )
        .unwrap();
        assert_eq!(
            redact_url(&url),
            "https://***@api.steampowered.com/ISteamRemoteStorage/x/?key=***&itemcount=1&access_token=***"
        );
    }

    #[test]
    fn leaves_plain_urls_untouched() {
        assert_eq!(
            redact_url_str("https://example.com/a?b=1&page=2"),
            "https://example.com/a?b=1&page=2"
        );
        assert_eq!(redact_url_str("not a url"), "not a url");
    }

    #[test]
    fn command_lines_mask_tokens_and_truncate_long_args() {
        let long = "x".repeat(400);
        let line = format_command_line(
            "git",
            [
                "clone",
                "https://bob:secret@github.com/a/b.git",
                "--token",
                "abc",
                "--api-key=zzz",
                "my dir",
                long.as_str(),
            ],
        );
        assert!(line.starts_with(
            "git clone https://***@github.com/a/b.git --token *** --api-key=*** \"my dir\" "
        ));
        assert!(!line.contains("secret"));
        assert!(!line.contains("abc"));
        assert!(line.contains("...(400 chars total)"));
    }

    #[test]
    fn error_chain_includes_sources_once() {
        #[derive(Debug)]
        struct Wrapper(std::io::Error);
        impl std::fmt::Display for Wrapper {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("request failed")
            }
        }
        impl std::error::Error for Wrapper {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let err = Wrapper(std::io::Error::other("disk full"));
        assert_eq!(error_chain(&err), "request failed: disk full");
    }
}
