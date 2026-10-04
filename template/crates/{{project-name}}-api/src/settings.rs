//! The configuration section the HTTP interface owns.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

/// `[server]`: the HTTP server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSettings {
    /// `server.http_addr`: the address to listen on.
    #[serde(deserialize_with = "svc_util::de::socket_addr")]
    pub http_addr: SocketAddr,
    /// `server.request_timeout`: a request that takes longer gets 503; 1s to 300s.
    #[serde(
        serialize_with = "svc_util::duration::serialize",
        deserialize_with = "svc_util::de::duration::<_, 1, 300>"
    )]
    pub request_timeout: Duration,
    /// `server.body_limit_bytes`: a larger request body gets 413; 1 to 67108864.
    #[serde(deserialize_with = "svc_util::de::integer::<_, 1, 67_108_864>")]
    pub body_limit_bytes: usize,
    /// `server.cors_origins`: the origins browsers may call the API from, such as
    /// `https://app.example.com`, or `*` alone for any; empty turns CORS off. Variables and
    /// flags give the list as text, separated by commas.
    #[serde(deserialize_with = "origins")]
    pub cors_origins: Vec<String>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        ServerSettings {
            http_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)),
            request_timeout: Duration::from_secs(15),
            body_limit_bytes: 1_048_576,
            cors_origins: Vec::new(),
        }
    }
}

/// A list of origins: a TOML array, or text separated by commas.
fn origins<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    struct OriginsVisitor;

    impl<'de> Visitor<'de> for OriginsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(r#"a list of origins like "https://app.example.com", or "*""#)
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<Vec<String>, E> {
            let items = text
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty());
            checked(items.map(str::to_string).collect())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                items.push(item);
            }
            checked(items)
        }
    }

    deserializer.deserialize_any(OriginsVisitor)
}

/// Every origin is `http` or `https`, a host and an optional port, without a path; `*`
/// stands alone.
fn checked<E: de::Error>(origins: Vec<String>) -> Result<Vec<String>, E> {
    if origins.iter().any(|origin| origin == "*") && origins.len() > 1 {
        return Err(E::custom(
            r#""*" allows every origin and cannot be combined with others"#,
        ));
    }
    for origin in origins.iter().filter(|origin| *origin != "*") {
        let rest = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"));
        let valid = rest.is_some_and(|host| {
            !host.is_empty()
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-.:[]".contains(c))
        });
        if !valid {
            return Err(E::custom(format!(
                "{origin:?} is not an origin like \"https://app.example.com\""
            )));
        }
    }
    Ok(origins)
}

#[cfg(test)]
mod tests {
    use super::ServerSettings;

    fn origins(value: &str) -> Result<Vec<String>, String> {
        let text = format!(
            "http_addr = \"127.0.0.1:8080\"\nrequest_timeout = \"15s\"\nbody_limit_bytes = 1\ncors_origins = {value}\n"
        );
        toml::from_str::<ServerSettings>(&text)
            .map(|settings| settings.cors_origins)
            .map_err(|error| error.message().to_string())
    }

    #[test]
    fn origins_come_as_an_array_or_as_text_separated_by_commas() {
        let both = Ok(vec![
            "https://a.example".to_string(),
            "http://b.example:8080".to_string(),
        ]);
        assert_eq!(
            origins(r#"["https://a.example", "http://b.example:8080"]"#),
            both
        );
        assert_eq!(
            origins(r#""https://a.example, http://b.example:8080""#),
            both
        );
        assert_eq!(origins(r#""""#), Ok(vec![]));
        assert_eq!(origins(r#""*""#), Ok(vec!["*".to_string()]));
    }

    #[test]
    fn anything_but_an_origin_is_rejected() {
        for bad in [
            r#""*, https://a.example""#,
            r#""ftp://a.example""#,
            r#""https://a.example/path""#,
            r#""a.example""#,
        ] {
            assert!(origins(bad).is_err(), "{bad}");
        }
    }
}
