//! HTTP for plugins, through `wasi:http`: sheaf makes the request (and the
//! TLS connection) and only to hosts the manifest lists under
//! `[permissions] network` and the user allowed. Anything else fails with
//! an error saying the request was denied.

use wasi::http::outgoing_handler;
use wasi::http::types::{ErrorCode, Fields, IncomingBody, Method, OutgoingRequest, Scheme};
use wasi::io::streams::StreamError;

/// Largest response body `get` reads.
pub const MAX_BODY: usize = 8 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Split `https://host[:port]/path?query` into its parts.
pub fn split_url(url: &str) -> Result<(Scheme, String, String), String> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        (Scheme::Https, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (Scheme::Http, r)
    } else {
        return Err(format!("not an http(s) URL: {url}"));
    };
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return Err(format!("no host in {url}"));
    }
    Ok((scheme, authority.to_string(), path))
}

fn describe(e: &ErrorCode) -> String {
    match e {
        ErrorCode::HttpRequestDenied => {
            "the request was denied: the host isn't one this plugin may reach".into()
        }
        other => format!("{other:?}"),
    }
}

/// Most redirects `get` follows.
pub const MAX_REDIRECTS: usize = 5;

/// GET a URL and read the whole body (up to [`MAX_BODY`]), following
/// redirects. Each hop must be to a host the plugin may reach.
pub fn get(url: &str) -> Result<Response, String> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        match get_once(&url)? {
            (r, Some(next)) if (300..400).contains(&r.status) => url = resolve(&url, &next),
            (r, _) => return Ok(r),
        }
    }
    Err(format!("too many redirects, last to {url}"))
}

/// `location` relative to `base`: absolute URLs as they are, `/path` on
/// the same host.
pub fn resolve(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    let host_end = base[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i)
        .unwrap_or(base.len());
    if location.starts_with('/') {
        format!("{}{location}", &base[..host_end])
    } else {
        let dir_end = base
            .rfind('/')
            .filter(|i| *i >= host_end)
            .unwrap_or(host_end);
        format!("{}/{location}", &base[..dir_end])
    }
}

/// One request: the response, and its `Location` if it has one.
fn get_once(url: &str) -> Result<(Response, Option<String>), String> {
    let (scheme, authority, path) = split_url(url)?;
    let req = OutgoingRequest::new(Fields::new());
    req.set_method(&Method::Get).map_err(|_| "bad method")?;
    req.set_scheme(Some(&scheme)).map_err(|_| "bad scheme")?;
    req.set_authority(Some(&authority))
        .map_err(|_| format!("bad host {authority}"))?;
    req.set_path_with_query(Some(&path))
        .map_err(|_| format!("bad path {path}"))?;
    let pending = outgoing_handler::handle(req, None).map_err(|e| describe(&e))?;
    let resp = loop {
        if let Some(r) = pending.get() {
            break r
                .map_err(|_| "response already taken".to_string())?
                .map_err(|e| describe(&e))?;
        }
        pending.subscribe().block();
    };
    let status = resp.status();
    let location = resp
        .headers()
        .get("location")
        .into_iter()
        .next()
        .map(|v| String::from_utf8_lossy(&v).into_owned());
    let body = resp.consume().map_err(|_| "no response body")?;
    let mut out = Vec::new();
    {
        let stream = body.stream().map_err(|_| "no response stream")?;
        loop {
            match stream.blocking_read(64 * 1024) {
                Ok(chunk) => {
                    out.extend_from_slice(&chunk);
                    if out.len() > MAX_BODY {
                        return Err("response too large".into());
                    }
                }
                Err(StreamError::Closed) => break,
                Err(StreamError::LastOperationFailed(e)) => {
                    return Err(format!("reading the response: {}", e.to_debug_string()));
                }
            }
        }
    }
    let _ = IncomingBody::finish(body);
    Ok((Response { status, body: out }, location))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split() {
        let (s, a, p) =
            split_url("https://api.frankfurter.app/2026-10-01?from=EUR&to=USD").unwrap();
        assert!(matches!(s, Scheme::Https));
        assert_eq!(a, "api.frankfurter.app");
        assert_eq!(p, "/2026-10-01?from=EUR&to=USD");
        let (s, a, p) = split_url("http://127.0.0.1:8080").unwrap();
        assert!(matches!(s, Scheme::Http));
        assert_eq!((a.as_str(), p.as_str()), ("127.0.0.1:8080", "/"));
        let (_, a, p) = split_url("http://h?q=1").unwrap();
        assert_eq!((a.as_str(), p.as_str()), ("h", "/?q=1"));
        assert!(split_url("ftp://x").is_err());
        assert_eq!(
            resolve("https://a.app/x?y", "https://b.dev/v1/x"),
            "https://b.dev/v1/x"
        );
        assert_eq!(resolve("https://a.app/v1/x", "/v2/y"), "https://a.app/v2/y");
        assert_eq!(resolve("https://a.app/v1/x", "z"), "https://a.app/v1/z");
        assert!(split_url("https:///x").is_err());
    }
}
