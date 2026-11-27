use clap::Parser;
use std::io::IsTerminal;
use winshortcutsorganizer::cli::{Cli, Command};
use winshortcutsorganizer::{banner, commands, console, shortcut::writer::ComGuard};

fn main() {
    console::use_utf8_output();

    let cli = Cli::parse();

    if banner::should_print(std::io::stdout().is_terminal(), cli.yes) {
        println!("{}\n", banner::header());
    }

    // Every shortcut read or write in this program goes through the shell
    // COM APIs, so COM must be initialized on this thread before any of
    // them run, and held for the whole process lifetime rather than
    // per-call: `_com`'s `Drop` uninitializes it exactly once, when `main`
    // returns.
    let _com = match ComGuard::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };

    let code = match cli.effective_command() {
        Command::Sync => commands::sync(&cli),
        Command::Migrate => commands::migrate(&cli),
        Command::Clean => commands::clean(&cli),
        Command::Status => commands::status(&cli),
        Command::Open => commands::open(&cli),
    };

    std::process::exit(code);
}
