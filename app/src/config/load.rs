use super::{Config, ConfigError, RawConfig, resolve};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
pub(crate) const DEFAULT_PATH: &str = "config/service.toml";

/// Resolve precedence from the entry point's snapshot, without reading globals.
/// The source label is safe to log; neither the path nor environment values are.
pub(crate) fn select_path(
    explicit: Option<PathBuf>,
    environment: &BTreeMap<OsString, OsString>,
    key: &OsStr,
) -> (PathBuf, &'static str) {
    if let Some(path) = explicit {
        (path, "cli")
    } else if let Some(path) = environment.get(key) {
        (path.into(), "environment")
    } else {
        (DEFAULT_PATH.into(), "default")
    }
}

#[derive(Clone)]
pub(crate) struct LoadContext {
    pub(crate) path: PathBuf,
    pub(crate) directory: PathBuf,
    pub(crate) environment: BTreeMap<OsString, OsString>,
    pub(crate) default_workers: usize,
}
impl LoadContext {
    pub(crate) fn new(
        path: PathBuf,
        cwd: &Path,
        environment: BTreeMap<OsString, OsString>,
        default_workers: usize,
    ) -> Result<Self, ConfigError> {
        if path.as_os_str().is_empty() {
            return Err(ConfigError::new("path", "configuration path is empty"));
        }
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        let directory = path
            .parent()
            .ok_or_else(|| ConfigError::new("path", "configuration path has no directory"))?
            .to_owned();
        Ok(Self {
            path,
            directory,
            environment,
            default_workers,
        })
    }
}

pub(crate) fn load(context: &LoadContext) -> Result<Config, ConfigError> {
    // Reject ordinary FIFO/device paths before open (a FIFO open can block).
    // Recheck the opened descriptor below; this is not a hard deadline guarantee
    // for a stalled filesystem or a malicious path-replacement race.
    if !std::fs::metadata(&context.path)
        .map_err(|_| ConfigError::new("file", "cannot inspect configuration file"))?
        .is_file()
    {
        return Err(ConfigError::new(
            "file",
            "configuration must be a regular file",
        ));
    }
    // The size bound applies to the actual read, not just a metadata length.
    let file = File::open(&context.path)
        .map_err(|_| ConfigError::new("file", "cannot open configuration file"))?;
    if !file
        .metadata()
        .map_err(|_| ConfigError::new("file", "cannot inspect configuration file"))?
        .is_file()
    {
        return Err(ConfigError::new(
            "file",
            "configuration must be a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ConfigError::new("file", "cannot read configuration file"))?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::new("file", "configuration exceeds 64 KiB"));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ConfigError::new("file", "configuration must be UTF-8"))?;
    parse(text, context)
}

pub(super) fn parse(text: &str, context: &LoadContext) -> Result<Config, ConfigError> {
    // I: TOML first, then expand only string values. Quotes/newlines from env
    // cannot become TOML syntax, and inserted values are never expanded again.
    // toml 1.x Value::from_str parses a standalone value, not a document.
    // Use the document deserializer explicitly (the empty document is valid).
    let mut value: toml::Value =
        toml::from_str(text).map_err(|_| ConfigError::new("document", "invalid TOML syntax"))?;
    expand_value(
        &mut value,
        "",
        &context.environment,
        &mut (MAX_CONFIG_BYTES as usize),
    )?;
    let raw: RawConfig = serde_path_to_error::deserialize(value).map_err(|error| {
        ConfigError::new(
            error.path().to_string(),
            "unknown field or invalid value type",
        )
    })?;
    // II / III: normalize/default, then component and cross-component validation.
    resolve(raw, context)
}

fn expand_value(
    value: &mut toml::Value,
    path: &str,
    env: &BTreeMap<OsString, OsString>,
    remaining: &mut usize,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => {
            *text = expand(text, path, env, *remaining)?;
            *remaining -= text.len();
        }
        toml::Value::Table(table) => {
            for (key, child) in table {
                expand_value(
                    child,
                    &if path.is_empty() {
                        key.to_owned()
                    } else {
                        format!("{path}.{key}")
                    },
                    env,
                    remaining,
                )?;
            }
        }
        toml::Value::Array(values) => {
            for (index, child) in values.iter_mut().enumerate() {
                expand_value(child, &format!("{path}[{index}]"), env, remaining)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn expand(
    text: &str,
    path: &str,
    env: &BTreeMap<OsString, OsString>,
    limit: usize,
) -> Result<String, ConfigError> {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            if result.len() + ch.len_utf8() > limit {
                return Err(ConfigError::new(
                    path,
                    "expanded configuration exceeds 64 KiB",
                ));
            }
            result.push(ch);
            continue;
        }
        match chars.next() {
            Some('$') => {
                if result.len() == limit {
                    return Err(ConfigError::new(
                        path,
                        "expanded configuration exceeds 64 KiB",
                    ));
                }
                result.push('$');
            }
            Some('{') => {
                let mut token = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => token.push(ch),
                        None => {
                            return Err(ConfigError::new(
                                path,
                                "unterminated environment placeholder",
                            ));
                        }
                    }
                }
                let (name, fallback) = token
                    .split_once(":-")
                    .map_or((token.as_str(), None), |(name, value)| (name, Some(value)));
                let valid = !name.is_empty()
                    && name.chars().enumerate().all(|(index, ch)| {
                        ch == '_' || ch.is_ascii_alphabetic() || (index > 0 && ch.is_ascii_digit())
                    });
                if !valid {
                    return Err(ConfigError::new(path, "invalid environment variable name"));
                }
                let value = env
                    .get(std::ffi::OsStr::new(name))
                    .map(|value| {
                        value
                            .to_str()
                            .ok_or_else(|| ConfigError::new(path, "environment value is not UTF-8"))
                    })
                    .transpose()?;
                let chosen = value
                    .filter(|value| !value.is_empty())
                    .or(fallback)
                    .ok_or_else(|| {
                        ConfigError::new(
                            path,
                            "environment variable is missing or empty and has no fallback",
                        )
                    })?;
                if chosen.len() > limit - result.len() {
                    return Err(ConfigError::new(
                        path,
                        "expanded configuration exceeds 64 KiB",
                    ));
                }
                result.push_str(chosen);
            }
            _ => {
                return Err(ConfigError::new(
                    path,
                    "use ${NAME}, ${NAME:-fallback}, or $$ for a literal dollar",
                ));
            }
        }
    }
    Ok(result)
}
