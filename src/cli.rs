use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "herdr-spreader",
    about = "Apply tmuxinator-style project layouts from YAML"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, PartialEq)]
pub enum Command {
    Apply {
        #[arg(long, short)]
        file: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Write the current herdr session (workspaces, tabs, pane split
    /// topology, pane working directories, and focus) to a YAML file that
    /// `apply` can consume.
    Export {
        #[arg(long, short)]
        file: PathBuf,
        /// Overwrite the output file if it already exists.
        #[arg(long)]
        force: bool,
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
            }
        );

        let cli = Cli::try_parse_from(["herdr-spreader", "apply"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: None,
                dry_run: false
            }
        );

        let cli = Cli::try_parse_from(["herdr-spreader", "apply", "-f", "./spread.yml"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: Some(PathBuf::from("./spread.yml")),
                dry_run: false,
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
                dry_run: true
            }
        );
        let cli =
            Cli::try_parse_from(["herdr-spreader", "apply", "--file", "x", "--dry-run"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: Some(PathBuf::from("x")),
                dry_run: true
            }
        );
        let cli = Cli::try_parse_from(["herdr-spreader", "apply"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Apply {
                file: None,
                dry_run: false
            }
        );
    }

    #[test]
    fn should_parse_export_subcommand_with_long_and_short_file_flags() {
        let cli = Cli::try_parse_from(["herdr-spreader", "export", "--file", "dev.yaml"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Export {
                file: PathBuf::from("dev.yaml"),
                force: false,
            }
        );

        let cli = Cli::try_parse_from(["herdr-spreader", "export", "-f", "dev.yaml"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Export {
                file: PathBuf::from("dev.yaml"),
                force: false,
            }
        );
    }

    #[test]
    fn should_parse_export_subcommand_with_force_flag() {
        let cli =
            Cli::try_parse_from(["herdr-spreader", "export", "--file", "dev.yaml", "--force"])
                .unwrap();
        assert_eq!(
            cli.command,
            Command::Export {
                file: PathBuf::from("dev.yaml"),
                force: true,
            }
        );
    }

    #[test]
    fn should_reject_export_subcommand_when_no_file_is_given() {
        let result = Cli::try_parse_from(["herdr-spreader", "export"]);
        assert!(
            result.is_err(),
            "export without --file should not parse: the output path is required"
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
