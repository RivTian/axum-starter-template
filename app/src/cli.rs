use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Help,
    Version,
    Run { config: Option<PathBuf> },
}

pub(crate) fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Action, &'static str> {
    let args: Vec<_> = arguments.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Action::Run { config: None }),
        [flag] if flag == "--help" || flag == "-h" => Ok(Action::Help),
        [flag] if flag == "--version" || flag == "-V" => Ok(Action::Version),
        [flag, path] if flag == "--config" && !path.is_empty() => Ok(Action::Run {
            config: Some(path.into()),
        }),
        _ => Err("expected --help, --version, or --config <path>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_one_action_and_does_not_silently_ignore_arguments() {
        assert_eq!(parse(Vec::new()).unwrap(), Action::Run { config: None });
        assert_eq!(parse(["--help".into()]).unwrap(), Action::Help);
        assert!(parse(["--version".into(), "secret".into()]).is_err());
        assert!(parse(["--config".into()]).is_err());
        assert_eq!(
            parse(["--config".into(), "service.toml".into()]).unwrap(),
            Action::Run {
                config: Some("service.toml".into())
            }
        );
    }
}
