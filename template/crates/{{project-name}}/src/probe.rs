//! `probe <URL>`: one plain HTTP/1.1 GET, for container health checks. It reads no
//! configuration and starts no logging; a 2xx answer exits 0 without output, anything else
//! writes one line to stderr and exits 1.

use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::cli::ProbeArgs;
use crate::exit::{self, Exit};
use crate::names::SERVICE_NAME;

/// The limit for the whole probe, every address included: below the 3 seconds the image's
/// health check allows, so the probe reports why it failed before it is killed.
const LIMIT: Duration = Duration::from_secs(2);

/// Runs `probe`.
pub(crate) fn run(args: &ProbeArgs) -> Exit {
    let reason = match get(&args.url) {
        Ok(status) if (200..300).contains(&status) => return Exit::OK,
        Ok(status) => format!("status {status}"),
        Err(reason) => reason,
    };
    exit::report(&format!("probe failed: {}: {reason}", args.url));
    Exit::PROBE_FAILED
}

/// Sends GET and returns the status code, or why there is none. Every address the host
/// resolves to is tried in turn, so `localhost` works whether the service listens on `::1`
/// or on `127.0.0.1`.
fn get(url: &str) -> Result<u16, String> {
    let (authority, path) = split_url(url).ok_or("invalid URL")?;
    let host_port = if authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.contains(']'))
    {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let addrs = host_port
        .to_socket_addrs()
        .map_err(|_| "invalid URL".to_string())?;
    let deadline = Instant::now() + LIMIT;
    let mut last = "the host has no address".to_string();
    for addr in addrs {
        let connected = left(deadline).and_then(|left| TcpStream::connect_timeout(&addr, left));
        match connected {
            Ok(stream) => {
                return exchange(stream, authority, path, deadline).map_err(|e| why(&e));
            }
            Err(error) => last = why(&error),
        }
    }
    Err(last)
}

/// The time left before the deadline; none left is a timeout.
fn left(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(ErrorKind::TimedOut.into())
    } else {
        Ok(left)
    }
}

fn exchange(
    mut stream: TcpStream,
    authority: &str,
    path: &str,
    deadline: Instant,
) -> io::Result<u16> {
    // Reads and writes end at the deadline, however long the earlier steps took.
    stream.set_write_timeout(Some(left(deadline)?))?;
    let agent = format!("{SERVICE_NAME}-probe");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: {authority}\r\nuser-agent: {agent}\r\nconnection: close\r\n\r\n"
    )?;
    stream.set_read_timeout(Some(left(deadline)?))?;
    let mut status_line = String::new();
    BufReader::new(stream).read_line(&mut status_line)?;
    status_of(&status_line).ok_or_else(|| io::Error::other("invalid response"))
}

/// `http://host:port/path` as `host:port` and `/path`.
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(slash) => rest.split_at(slash),
        None => (rest, "/"),
    };
    (!authority.is_empty()).then_some((authority, path))
}

/// The code of a status line such as `HTTP/1.1 200 OK`.
fn status_of(line: &str) -> Option<u16> {
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    let code = parts.next()?;
    if !version.starts_with("HTTP/") || code.len() != 3 {
        return None;
    }
    code.parse().ok()
}

fn why(error: &io::Error) -> String {
    match error.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => "timed out".to_string(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{split_url, status_of};

    #[test]
    fn urls_split_into_authority_and_path() {
        assert_eq!(
            split_url("http://127.0.0.1:8080/readyz"),
            Some(("127.0.0.1:8080", "/readyz"))
        );
        assert_eq!(split_url("http://[::1]:8080"), Some(("[::1]:8080", "/")));
        assert_eq!(split_url("http://localhost"), Some(("localhost", "/")));
        assert_eq!(split_url("https://127.0.0.1/readyz"), None);
        assert_eq!(split_url("http:///readyz"), None);
    }

    #[test]
    fn status_lines_give_their_code() {
        assert_eq!(status_of("HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(status_of("HTTP/1.1 503 Service Unavailable\r\n"), Some(503));
        assert_eq!(status_of("SSH-2.0-OpenSSH\r\n"), None);
        assert_eq!(status_of(""), None);
    }
}
