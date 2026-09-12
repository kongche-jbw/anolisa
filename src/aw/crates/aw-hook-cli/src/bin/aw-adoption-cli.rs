//! Explicit observation writes and read-only adoption queries.

use std::{io::Write, path::Path};

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--help"] {
        println!("Usage: aw-adoption-cli <observe|query> /absolute/CONTEXT.json\nobserve records one native history snapshot; query only reads retained evidence.");
        return;
    }
    let result = match arguments.as_slice() {
        [command, path] if command == "observe" => aw_hook_cli::observe(Path::new(path)),
        [command, path] if command == "query" => aw_hook_cli::query(Path::new(path)),
        _ => Err(aw_hook_cli::Error::Input),
    };
    let result = result.and_then(|value| {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{value}")
            .and_then(|_| stdout.flush())
            .map_err(|_| aw_hook_cli::Error::Execution)
    });
    if let Err(error) = result {
        eprintln!("aw-adoption: {error}");
        std::process::exit(1);
    }
}
