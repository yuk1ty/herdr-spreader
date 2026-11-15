use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;

use herdr_spreader::backend::cli::CliBackend;
use herdr_spreader::backend::{ExistingWorkspaces, HerdrBackend, WorkspaceSummary};
use herdr_spreader::cli::{Cli, Command};
use herdr_spreader::config::{read_config, resolve_config_path, resolve_paths};
use herdr_spreader::engine;
use herdr_spreader::validate;

fn main() -> anyhow::Result<()> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let cli = Cli::parse();

    match cli.command {
        Command::Apply { file, dry_run } => {
            let config_path = resolve_config_path(file, &env)?;
            let contents = read_config(&config_path)?;
            let spread_file = match validate::validate_config(validate::SourceFile {
                yaml: &contents,
                path: &config_path,
            }) {
                Ok(f) => f,
                Err(findings) => {
                    validate::print_findings(&findings);
                    std::process::exit(1);
                }
            };

            let bin = CliBackend::resolve_bin(&env);
            let socket_path = env.get("HERDR_SOCKET_PATH").map(PathBuf::from);
            let mut backend = CliBackend::new(bin, socket_path);

            let cwd = match env
                .get("HERDR_PANE_ID")
                .and_then(|id| backend.query_pane_cwd(id))
            {
                Some(cwd) => cwd,
                None => std::env::current_dir()?,
            };
            let existing = ExistingWorkspaces::from_list(
                backend
                    .list_workspaces()?
                    .into_iter()
                    .map(WorkspaceSummary::from),
            );
            let spread_file = resolve_paths(&spread_file, &env, &cwd);

            let plan = engine::plan_file(&spread_file, &existing);
            warn_skipped(&plan);

            if dry_run {
                for op in &plan {
                    println!("{}", engine::render_op(op));
                }
                return Ok(());
            }

            engine::execute_plan(&plan, &mut backend)?;

            Ok(())
        }
    }
}

fn warn_skipped(plan: &[engine::BackendOp]) {
    for op in plan {
        if let engine::BackendOp::SkipWorkspace { label } = op {
            eprintln!(
                "warning: skipping workspace '{label}' (already exists); its tabs, panes, and commands are left untouched"
            );
        }
    }
}
