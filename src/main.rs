use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;

use herdr_spreader::backend::cli::CliBackend;
use herdr_spreader::cli::{Cli, Command};
use herdr_spreader::config::{read_config, resolve_config_path, resolve_paths};
use herdr_spreader::engine;
use herdr_spreader::validate;

fn main() -> anyhow::Result<()> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let cli = Cli::parse();

    match cli.command {
        Command::Apply {
            file,
            dry_run,
            on_existing,
        } => {
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
            let spread_file = resolve_paths(&spread_file, &env, &cwd);

            let on_existing = engine::OnExisting::from(on_existing);

            if dry_run {
                // Reading the server state is what makes a dry run under
                // `--on-existing skip`/`sync` show the plan that would really
                // run. Under the default it reads nothing, so a dry run still
                // spawns no herdr process at all.
                let state = engine::read_existing_state(&spread_file, on_existing, &mut backend)?;
                let plan = engine::plan_file_with_state(&spread_file, &state, on_existing);
                for op in &plan {
                    println!("{}", engine::render_op(op));
                }
                return Ok(());
            }

            // Report before running, so a long-running layout says what it is
            // about to do rather than going quiet; the summary is derived from
            // the same plan that then executes, so it cannot disagree with it.
            let state = engine::read_existing_state(&spread_file, on_existing, &mut backend)?;
            for line in engine::summarize(&spread_file, &state, on_existing) {
                println!("{}", line.render());
            }

            let plan = engine::plan_file_with_state(&spread_file, &state, on_existing);
            engine::execute_plan(&plan, &mut backend)?;

            Ok(())
        }
    }
}
