// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "main.rs"
// ============================================================================
// crates/cli/src/main.rs
//! `ubel`: the Ubel Stratum command-line toolchain.
//!
//! Layout mirrors the `mdix` CLI in DixScript-Rust: `commands/` holds one
//! file per subcommand (clap `Args` plus a `run` function returning an exit
//! code), `services/` holds the logic those commands share, `output/` holds
//! terminal and JSON presentation. Only `check`, `run` and `debug-tokens`
//! exist so far.

mod commands;
mod output;
mod services;

use clap::{Parser, Subcommand};
use commands::{check::CheckArgs, debug_tokens::DebugTokensArgs, run::RunArgs, GlobalOpts};

#[derive(Parser)]
#[command(
    name    = "ubel",
    version = env!("CARGO_PKG_VERSION"),
    about   = "Ubel Stratum (.ubl) toolchain",
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Print extra detail (token counts, timings)
    #[arg(long, global = true)]
    pub verbose: bool,
    /// Suppress success output
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Machine-readable JSON on stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Disable ANSI colors
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Lex, parse and type-check a .ubl file without running it
    Check(CheckArgs),
    /// Check a .ubl file, then run its `main`
    Run(RunArgs),
    /// [DEBUG] Print the token stream with positions
    ///
    /// Output format is internal and not guaranteed stable across versions.
    #[command(name = "debug-tokens")]
    DebugTokens(DebugTokensArgs),
}

fn main() {
    let cli = Cli::parse();

    if cli.no_color {
        colored::control::set_override(false);
    }

    let global = GlobalOpts { verbose: cli.verbose, quiet: cli.quiet, json: cli.json };

    let code = match cli.command {
        Commands::Check(args)       => commands::check::run(args, &global),
        Commands::Run(args)         => commands::run::run(args, &global),
        Commands::DebugTokens(args) => commands::debug_tokens::run(args, &global),
    };

    std::process::exit(code);
}
