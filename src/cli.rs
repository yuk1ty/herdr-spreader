use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::engine;

#[derive(Debug, Parser)]
#[command(
    name = "herdr-spreader",
    about = "Apply tmuxinator-style project layouts from YAML"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// What to do about a workspace whose label already exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OnExistingArg {
    /// Build the layout anyway, producing a second workspace with the same
    /// label. The behaviour this tool has always had.
    #[default]
    Create,
    /// Leave an existing workspace untouched and build nothing for it.
    Skip,
    /// Keep an existing workspace and add only the tabs it is missing, matched
    /// by label.
    Sync,
}

impl From<OnExistingArg> for engine::OnExisting {
    fn from(value: OnExistingArg) -> Self {
        match value {
            OnExistingArg::Create => Self::Create,
            OnExistingArg::Skip => Self::Skip,
            OnExistingArg::Sync => Self::Sync,
        }
    }
}

#[derive(Debug, Subcommand, PartialEq)]
pub enum Command {
    Apply {
        #[arg(long, short)]
        file: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        /// What to do about a workspace whose label already exists.
        #[arg(long, value_enum, default_value_t = OnExistingArg::Create)]
        on_existing: OnExistingArg,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_apply_subcommand_with_optional_config_file_argument() {
        let cli =
            Cli::try_parse_from(["herdr-spreader", "apply", "--file", "./spread.yml"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: Some(PathBuf::from("./spread.yml")),
                dry_run: false,
                on_existing: OnExistingArg::Create,
            }
        );

        let cli = Cli::try_parse_from(["herdr-spreader", "apply"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: None,
                dry_run: false,
                on_existing: OnExistingArg::Create,
            }
        );

        let cli = Cli::try_parse_from(["herdr-spreader", "apply", "-f", "./spread.yml"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: Some(PathBuf::from("./spread.yml")),
                dry_run: false,
                on_existing: OnExistingArg::Create,
            }
        );
    }

    #[test]
    fn should_parse_apply_subcommand_with_dry_run_flag() {
        let cli = Cli::try_parse_from(["herdr-spreader", "apply", "--dry-run"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: None,
                dry_run: true,
                on_existing: OnExistingArg::Create,
            }
        );
        let cli =
            Cli::try_parse_from(["herdr-spreader", "apply", "--file", "x", "--dry-run"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: Some(PathBuf::from("x")),
                dry_run: true,
                on_existing: OnExistingArg::Create,
            }
        );
        let cli = Cli::try_parse_from(["herdr-spreader", "apply"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: None,
                dry_run: false,
                on_existing: OnExistingArg::Create,
            }
        );
    }

    #[test]
    fn should_reject_validate_subcommand() {
        let result = Cli::try_parse_from(["herdr-spreader", "validate"]);
        assert!(
            result.is_err(),
            "validate subcommand should no longer be parsed"
        );
    }
}
