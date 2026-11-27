//! Cargo installs this frontend and the low-level evaluator beside one another.
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use sha2::{Digest, Sha256};

const MANIFEST: &str = include_str!("../package.json");
const LOCKFILE: &str = include_str!("../package-lock.json");
const BUILDER: &str = include_str!("../tools/build.mjs");
const WRAPPER: &str = include_str!("../bin/rulecheck.mjs");

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("rulecheck: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<u8, Box<dyn std::error::Error>> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let evaluator = env::current_exe()?.with_file_name("rulecheck-evaluator");
    if !evaluator.is_file() {
        return Err(format!(
            "missing {}; reinstall Rulecheck with Cargo without a --bin filter",
            evaluator.display()
        )
        .into());
    }
    let first = args.first().and_then(|value| value.to_str());
    // These evaluator commands don't need Node or the TypeScript installation.
    if matches!(first, Some("schema" | "-V" | "--version")) {
        return run_child(Command::new(evaluator).args(&args), "rulecheck-evaluator");
    }

    // Use a private temporary directory for embedded scripts, never a directory
    // in the repository being checked. The wrapper retains the caller's cwd.
    let runtime = tempfile::tempdir()?;
    let path = runtime.path();
    for dir in ["bin", "tools", "target/release"] {
        fs::create_dir_all(path.join(dir))?;
    }
    fs::write(path.join("bin/rulecheck.mjs"), WRAPPER)?;
    fs::write(path.join("tools/build.mjs"), BUILDER)?;
    fs::write(path.join("package.json"), MANIFEST)?;
    symlink(evaluator, path.join("target/release/rulecheck-evaluator"))?;

    if matches!(first, Some("check" | "list" | "test" | "validate")) {
        let modules = compiler_modules()?;
        symlink(modules, path.join("node_modules"))?;
    }
    let mut node = js_command("node");
    node.arg(path.join("bin/rulecheck.mjs")).args(args);
    run_child(&mut node, "Node.js (install Node 20 or newer)")
}

fn run_child(command: &mut Command, program: &str) -> Result<u8, Box<dyn std::error::Error>> {
    let status = command
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    Ok(match status.code() {
        Some(0) => 0,
        Some(1) => 1,
        _ => 2,
    })
}

fn js_command(program: &str) -> Command {
    let mut command = Command::new(program);
    // Don't inherit JS preloads or module resolution from a checked project.
    command.env_remove("NODE_OPTIONS").env_remove("NODE_PATH");
    command
}

fn compiler_modules() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))
        .filter(|p| p.is_absolute())
        .ok_or("set HOME or an absolute XDG_CACHE_HOME to cache the pinned TypeScript compiler")?;
    let base = base.join("rulecheck");
    fs::create_dir_all(&base)?;
    let key = format!(
        "compiler-{:x}",
        Sha256::digest([MANIFEST, LOCKFILE].concat())
    );
    let cache = base.join(key);
    let modules = cache.join("node_modules");
    if modules.join("typescript/package.json").is_file() {
        return Ok(modules);
    }

    let setup = tempfile::tempdir_in(&base)?;
    let path = setup.path();
    fs::write(path.join("package.json"), MANIFEST)?;
    fs::write(path.join("package-lock.json"), LOCKFILE)?;
    fs::write(path.join("user.npmrc"), "")?;
    fs::write(path.join("global.npmrc"), "")?;
    eprintln!("rulecheck: setting up the pinned TypeScript compiler (first run only)");
    let mut npm = js_command("npm");
    for (name, _) in env::vars_os() {
        let name_text = name.to_string_lossy().to_ascii_lowercase();
        if name_text.starts_with("npm_config_") || name == OsStr::new("NODE_AUTH_TOKEN") {
            npm.env_remove(name);
        }
    }
    let result = npm
        .args([
            "ci",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--registry=https://registry.npmjs.org/",
            "--fetch-retries=1",
            "--fetch-timeout=30000",
            "--loglevel=error",
        ])
        .arg("--userconfig")
        .arg(path.join("user.npmrc"))
        .arg("--globalconfig")
        .arg(path.join("global.npmrc"))
        .arg("--cache")
        .arg(path.join("npm-cache"))
        .current_dir(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run npm for the first-run setup: {e}"))?;
    if !result.status.success() {
        io::stderr().write_all(&result.stderr)?;
        return Err("TypeScript setup failed; check npm and network access, then retry".into());
    }
    fs::remove_dir_all(path.join("npm-cache"))?;
    match fs::rename(path, &cache) {
        Ok(()) => {}
        // Another process completed setup while this one was installing.
        Err(_) if modules.join("typescript/package.json").is_file() => {}
        Err(error) => {
            return Err(format!("could not install into {}: {error}", cache.display()).into());
        }
    }
    Ok(modules)
}
