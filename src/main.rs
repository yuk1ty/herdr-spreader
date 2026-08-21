use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;

use herdr_spreader::backend::cli::CliBackend;
use herdr_spreader::cli::{Cli, Command};
use herdr_spreader::config::resolve_config_path;
use herdr_spreader::engine;
use herdr_spreader::include;
use herdr_spreader::validate;

fn main() -> anyhow::Result<()> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let cli = Cli::parse();

    match cli.command {
        Command::Apply { file, dry_run } => {
            let config_path = resolve_config_path(file, &env)?;

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
            // Paths in the root file resolve against the invocation directory, as
            // they always have; each included file resolves its own against its
            // own directory, which is what makes a repository-local layout
            // portable. Both happen inside the loader.
            let spread_file = match include::load_flat(&config_path, &cwd, &env) {
                Ok(f) => f,
                Err(findings) => {
                    validate::print_findings(&findings);
                    std::process::exit(1);
                }
            };

            if dry_run {
                let plan = engine::plan_file(&spread_file);
                for op in &plan {
                    println!("{}", engine::render_op(op));
                }
                return Ok(());
            }

            engine::apply(&spread_file, &mut backend)?;

            Ok(())
        }
    }
}
