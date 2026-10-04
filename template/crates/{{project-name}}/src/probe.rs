//! `probe <URL>`: one plain HTTP/1.1 GET, for container health checks. It reads no
//! configuration and starts no logging; a 2xx answer exits 0 without output, anything else
//! writes one line to stderr and exits 1.

use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::cli::ProbeArgs;
use crate::exit::{self, Exit};
use crate::names::SERVICE_NAME;

/// The limit for connecting, and for each read and write.
const TIMEOUT: Duration = Duration::from_secs(5);

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
    let mut last = "the host has no address".to_string();
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, TIMEOUT) {
            Ok(stream) => return exchange(stream, authority, path).map_err(|e| why(&e)),
            Err(error) => last = why(&error),
        }
    }
    Err(last)
}

fn exchange(mut stream: TcpStream, authority: &str, path: &str) -> io::Result<u16> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let agent = format!("{SERVICE_NAME}-probe");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: {authority}\r\nuser-agent: {agent}\r\nconnection: close\r\n\r\n"
    )?;
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
