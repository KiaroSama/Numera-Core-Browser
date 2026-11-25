use serde_json::Value;
use std::{
    fmt,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use url::Url;

const MAX_BODY: usize = 4 * 1024 * 1024;
const GITHUB_API: &str = "api.github.com";

#[derive(Debug)]
pub enum HttpError {
    Origin,
    Transport,
    Status(u16),
    Json(serde_json::Error),
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Origin => formatter.write_str("Metadata request left the permitted HTTPS origin"),
            Self::Transport => formatter.write_str("Metadata request did not finish successfully"),
            Self::Status(status) => write!(formatter, "Metadata endpoint returned HTTP {status}"),
            Self::Json(error) => write!(formatter, "Metadata response is invalid JSON: {error}"),
        }
    }
}

impl std::error::Error for HttpError {}

fn allowed(url: &Url, host: &str) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
}

pub fn request_json(
    source: &str,
    user_agent: &str,
    timeout: Duration,
    token: Option<&str>,
    data: Option<&Value>,
) -> Result<Value, HttpError> {
    let mut url = Url::parse(source).map_err(|_| HttpError::Origin)?;
    let host = url.host_str().ok_or(HttpError::Origin)?.to_owned();
    if !allowed(&url, &host) || (token.is_some() && (host != GITHUB_API || data.is_some())) {
        return Err(HttpError::Origin);
    }
    let deadline = Instant::now() + timeout;
    for _ in 0..=5 {
        let seconds = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64();
        if seconds < 0.001 {
            return Err(HttpError::Transport);
        }
        let (status, redirect, body) = request(&url, user_agent, seconds, token, data)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let destination = url.join(&redirect).map_err(|_| HttpError::Origin)?;
            if !allowed(&destination, &host) || redirect.is_empty() {
                return Err(HttpError::Origin);
            }
            url = destination;
        } else if (200..300).contains(&status) {
            return serde_json::from_slice(&body).map_err(HttpError::Json);
        } else {
            return Err(HttpError::Status(status));
        }
    }
    Err(HttpError::Transport)
}

fn request(
    url: &Url,
    user_agent: &str,
    seconds: f64,
    token: Option<&str>,
    data: Option<&Value>,
) -> Result<(u16, String, Vec<u8>), HttpError> {
    let mut command = Command::new("curl");
    command
        .args(["--disable", "--silent", "--proto", "=https", "--max-time"])
        .arg(format!("{seconds:.3}"))
        .arg("--max-filesize")
        .arg(MAX_BODY.to_string())
        .args([
            "--write-out",
            "\n%{http_code}\n%{redirect_url}",
            "--user-agent",
            user_agent,
        ])
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let input = if let Some(token) = token {
        if token.is_empty()
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~+/=".contains(&byte))
        {
            return Err(HttpError::Origin);
        }
        // A config line keeps the token out of process arguments. No
        // automatic redirects can reuse the header with a different origin.
        command.args(["--config", "-"]);
        format!("header = \"Authorization: Bearer {token}\"\n").into_bytes()
    } else if let Some(data) = data {
        command.args([
            "--header",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ]);
        serde_json::to_vec(data).map_err(HttpError::Json)?
    } else {
        Vec::new()
    };
    let mut child = command
        .arg("--")
        .arg(url.as_str())
        .spawn()
        .map_err(|_| HttpError::Transport)?;
    let written = child
        .stdin
        .take()
        .ok_or(HttpError::Transport)?
        .write_all(&input);
    let output = child.wait_with_output().map_err(|_| HttpError::Transport)?;
    if written.is_err() || !output.status.success() {
        return Err(HttpError::Transport);
    }
    let mut parts = output.stdout.rsplitn(3, |byte| *byte == b'\n');
    let redirect = parts
        .next()
        .and_then(|part| std::str::from_utf8(part).ok())
        .ok_or(HttpError::Transport)?;
    let status = parts
        .next()
        .and_then(|part| std::str::from_utf8(part).ok())
        .and_then(|code| code.parse().ok())
        .ok_or(HttpError::Transport)?;
    let body = parts
        .next()
        .filter(|body| body.len() <= MAX_BODY)
        .ok_or(HttpError::Transport)?;
    Ok((status, redirect.to_owned(), body.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::{GITHUB_API, HttpError, allowed, request_json};
    use std::time::Duration;
    use url::Url;

    #[test]
    fn accepts_only_the_exact_https_origin() {
        assert!(allowed(
            &Url::parse("https://api.github.com:443/repos/abc/def").unwrap(),
            GITHUB_API
        ));
        for address in [
            "http://api.github.com/path",
            "https://api.github.com.example.invalid/path",
            "https://api.github.com:444/path",
            "https://metadata@api.github.com/path",
            "https://example.invalid/path",
        ] {
            assert!(!allowed(&Url::parse(address).unwrap(), GITHUB_API));
        }
    }

    #[test]
    fn rejects_credentials_on_foreign_requests_without_calling_the_transport() {
        assert!(matches!(
            request_json(
                "https://example.invalid/path",
                "Numera",
                Duration::from_secs(1),
                Some("fixture"),
                None
            ),
            Err(HttpError::Origin)
        ));
    }
}
