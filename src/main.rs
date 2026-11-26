use std::process::ExitCode;

const HELP: &str = concat!(
    "rulecheck: deterministic repository checks\n\n",
    "This is a starter; rule evaluation is not implemented yet.",
);

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if args.next().is_some() {
        eprintln!("Expected a single flag. Run rulecheck --help.");
        return ExitCode::from(2);
    }

    match first.as_deref().and_then(std::ffi::OsStr::to_str) {
        None if first.is_none() => println!("{HELP}"),
        Some("-h" | "--help") => println!("{HELP}"),
        Some("-V" | "--version") => println!("rulecheck {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("Unknown command or flag. Run rulecheck --help.");
            return ExitCode::from(2);
        }
    }

    ExitCode::SUCCESS
}
