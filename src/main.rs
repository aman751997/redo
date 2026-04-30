use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use redo::cli::{self, Cli, Command};

fn main() -> Result<()> {
    redo::panic_hook::install();
    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        Command::Record { root } => {
            let root = cli::resolve_root(root)?;
            let cfg = redo::recorder::Config {
                root,
                session_id: None,
                print_banner: true,
                stop: None,
            };
            redo::recorder::run(cfg)?;
            Ok(())
        }
        Command::List { root } => {
            let root = cli::resolve_root(root)?;
            let summaries = cli::list::collect(&root)?;
            cli::list::print(&summaries);
            Ok(())
        }
        Command::Replay { session_id, root } => {
            let root = cli::resolve_root(root)?;
            let id = Uuid::parse_str(&session_id).context("parse session id as UUID")?;
            redo::tui::run(&root, id)
        }
        Command::Inspect { session_id, root } => {
            let root = cli::resolve_root(root)?;
            let id = Uuid::parse_str(&session_id).context("parse session id as UUID")?;
            cli::inspect::run(&root, id)
        }
        Command::Fork {
            session_id,
            at,
            label,
            root,
        } => {
            let root = cli::resolve_root(root)?;
            let id = Uuid::parse_str(&session_id).context("parse session id as UUID")?;
            let new_id = cli::fork::run(&root, id, at, label)?;
            println!("{new_id}");
            Ok(())
        }
        Command::Hook { kind, session_dir } => redo::hook::run(&kind, session_dir).map(|_| ()),
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("REDO_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
