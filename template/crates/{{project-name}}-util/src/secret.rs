//! Values that must never appear in logs, error messages or serialized output.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// What a [`Secret`] shows instead of its value.
pub const REDACTED: &str = "<redacted>";

/// A value, such as a password or a token, that prints and serializes as `<redacted>`.
/// Deserializing reads the real value; [`Secret::expose`] is the only way to get it back.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wraps a value.
    pub const fn new(value: T) -> Self {
        Secret(value)
    }

    /// The real value. Call it only where the value is used, never to log it.
    pub const fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> Serialize for Secret<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Secret)
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn never_shows_the_value() {
        let secret = Secret::new("hunter2".to_string());
        assert_eq!(secret.to_string(), "<redacted>");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn serializes_as_redacted_and_deserializes_the_value() -> Result<(), Box<dyn std::error::Error>>
    {
        let written = toml::Value::try_from(Secret::new("hunter2".to_string()))?;
        assert_eq!(written, toml::Value::String("<redacted>".to_string()));
        let read: Secret<String> =
            serde::Deserialize::deserialize(toml::Value::String("hunter2".to_string()))?;
        assert_eq!(read.expose(), "hunter2");
        Ok(())
    }
}
