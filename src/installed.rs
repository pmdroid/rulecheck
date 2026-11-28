//! Cargo installs this frontend and the low-level evaluator beside one another.
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use sha2::{Digest, Sha256, Sha512};

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

    if matches!(
        first,
        Some("check" | "baseline" | "list" | "test" | "validate")
    ) {
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

/// Where the first-run setup may fetch the pinned compiler from. All of it comes
/// from the environment of the person or CI job running Rulecheck, never from the
/// repository being checked, and a configured source is the only one tried.
enum Source {
    Public,
    Mirror {
        registry: String,
        userconfig: Option<PathBuf>,
    },
    Tarball(PathBuf),
}

const PUBLIC_REGISTRY: &str = "https://registry.npmjs.org/";

fn setting(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn compiler_source() -> Result<Source, Box<dyn std::error::Error>> {
    let registry = setting("RULECHECK_NPM_REGISTRY");
    let userconfig = setting("RULECHECK_NPM_USERCONFIG");
    let tarball = setting("RULECHECK_COMPILER_TARBALL");
    if let Some(tarball) = tarball {
        if registry.is_some() || userconfig.is_some() {
            return Err(
                "set either RULECHECK_COMPILER_TARBALL or RULECHECK_NPM_REGISTRY, not both".into(),
            );
        }
        let path = PathBuf::from(tarball);
        if !path.is_absolute() || !path.is_file() {
            return Err(format!(
                "RULECHECK_COMPILER_TARBALL must be the absolute path of an existing file, not {}",
                path.display()
            )
            .into());
        }
        return Ok(Source::Tarball(path));
    }
    let Some(registry) = registry else {
        if userconfig.is_some() {
            return Err("RULECHECK_NPM_USERCONFIG needs RULECHECK_NPM_REGISTRY".into());
        }
        return Ok(Source::Public);
    };
    let registry = registry
        .into_string()
        .map_err(|_| "RULECHECK_NPM_REGISTRY must be valid UTF-8")?;
    let rest = registry
        .strip_prefix("https://")
        .or_else(|| registry.strip_prefix("http://"))
        .ok_or("RULECHECK_NPM_REGISTRY must be an http:// or https:// URL")?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') || rest.contains(['?', '#']) {
        return Err("RULECHECK_NPM_REGISTRY must be a plain registry URL; put credentials in the file named by RULECHECK_NPM_USERCONFIG".into());
    }
    let registry = if registry.ends_with('/') {
        registry
    } else {
        format!("{registry}/")
    };
    let userconfig = match userconfig {
        None => None,
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() || !path.is_file() {
                return Err(format!(
                    "RULECHECK_NPM_USERCONFIG must be the absolute path of an existing file, not {}",
                    path.display()
                )
                .into());
            }
            let path = fs::canonicalize(&path)?;
            if path.starts_with(checked_repository()?) {
                return Err(
                    "RULECHECK_NPM_USERCONFIG must be outside the repository being checked".into(),
                );
            }
            Some(path)
        }
    };
    Ok(Source::Mirror {
        registry,
        userconfig,
    })
}

/// The Git work tree around the current directory, or the directory itself.
fn checked_repository() -> io::Result<PathBuf> {
    let cwd = env::current_dir()?;
    let toplevel = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| PathBuf::from(text.trim_end_matches('\n')));
    fs::canonicalize(toplevel.unwrap_or(cwd))
}

/// The `sha512-…` integrity that Rulecheck's lockfile pins for TypeScript.
fn pinned_integrity() -> Result<String, Box<dyn std::error::Error>> {
    let lock: serde_json::Value = serde_json::from_str(LOCKFILE)?;
    lock["packages"]["node_modules/typescript"]["integrity"]
        .as_str()
        .filter(|value| value.starts_with("sha512-"))
        .map(str::to_owned)
        .ok_or_else(|| "the embedded lockfile has no sha512 integrity for TypeScript".into())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                TABLE[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

fn npm_command(dir: &Path) -> Command {
    let mut npm = js_command("npm");
    // Only the flags below configure npm: no npm_config_* or token variables,
    // no user or global npmrc, and no .npmrc from the checked repository.
    for (name, _) in env::vars_os() {
        let name_text = name.to_string_lossy().to_ascii_lowercase();
        if name_text.starts_with("npm_config_") || name == OsStr::new("NODE_AUTH_TOKEN") {
            npm.env_remove(name);
        }
    }
    npm.args(["--no-audit", "--no-fund", "--loglevel=error"])
        .arg("--globalconfig")
        .arg(dir.join("global.npmrc"))
        .arg("--cache")
        .arg(dir.join("npm-cache"))
        .current_dir(dir)
        .stdin(Stdio::null());
    npm
}

fn run_npm(npm: &mut Command) -> Result<bool, Box<dyn std::error::Error>> {
    let result = npm
        .output()
        .map_err(|e| format!("could not run npm for the first-run setup: {e}"))?;
    if !result.status.success() {
        io::stderr().write_all(&result.stderr)?;
    }
    Ok(result.status.success())
}

fn install(path: &Path, source: &Source) -> Result<(), Box<dyn std::error::Error>> {
    let empty = path.join("user.npmrc");
    match source {
        Source::Public | Source::Mirror { .. } => {
            let (registry, userconfig) = match source {
                Source::Mirror {
                    registry,
                    userconfig,
                } => (registry.as_str(), userconfig.as_deref().unwrap_or(&empty)),
                _ => (PUBLIC_REGISTRY, empty.as_path()),
            };
            let mut npm = npm_command(path);
            // replace-registry-host sends the lockfile's registry.npmjs.org
            // tarball URL to the configured registry, so nothing reaches public npm.
            npm.args(["ci", "--ignore-scripts", "--replace-registry-host=npmjs"])
                .args(["--fetch-retries=1", "--fetch-timeout=30000"])
                .arg(format!("--registry={registry}"))
                .arg("--userconfig")
                .arg(userconfig);
            if run_npm(&mut npm)? {
                return Ok(());
            }
            Err(match source {
                Source::Public => format!(
                    "TypeScript setup from {PUBLIC_REGISTRY} failed; check npm and network access, \
                     or set RULECHECK_NPM_REGISTRY or RULECHECK_COMPILER_TARBALL, then retry"
                ),
                _ => format!(
                    "TypeScript setup from RULECHECK_NPM_REGISTRY {registry} failed; \
                     Rulecheck does not fall back to the public npm registry"
                ),
            }
            .into())
        }
        Source::Tarball(tarball) => {
            let expected = pinned_integrity()?;
            let actual = format!("sha512-{}", base64(&Sha512::digest(fs::read(tarball)?)));
            if actual != expected {
                return Err(format!(
                    "RULECHECK_COMPILER_TARBALL {} does not match the pinned TypeScript \
                     {}: expected {expected}, got {actual}",
                    tarball.display(),
                    pinned_version()
                )
                .into());
            }
            // Seed a private npm cache with the verified tarball, then install
            // from that cache only. npm checks the lockfile integrity again.
            let staged = path.join("typescript.tgz");
            fs::copy(tarball, &staged)?;
            let mut add = npm_command(path);
            add.args(["cache", "add", "--offline"])
                .arg(&staged)
                .arg("--userconfig")
                .arg(&empty);
            let mut ci = npm_command(path);
            ci.args(["ci", "--ignore-scripts", "--offline"])
                .arg(format!("--registry={PUBLIC_REGISTRY}"))
                .arg("--userconfig")
                .arg(&empty);
            if run_npm(&mut add)? && run_npm(&mut ci)? {
                fs::remove_file(staged)?;
                return Ok(());
            }
            Err("offline TypeScript setup from RULECHECK_COMPILER_TARBALL failed".into())
        }
    }
}

fn pinned_version() -> String {
    serde_json::from_str::<serde_json::Value>(MANIFEST)
        .ok()
        .and_then(|manifest| {
            manifest["dependencies"]["typescript"]
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_default()
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

    let source = compiler_source()?;
    let setup = tempfile::tempdir_in(&base)?;
    let path = setup.path();
    fs::write(path.join("package.json"), MANIFEST)?;
    fs::write(path.join("package-lock.json"), LOCKFILE)?;
    fs::write(path.join("user.npmrc"), "")?;
    fs::write(path.join("global.npmrc"), "")?;
    eprintln!("rulecheck: setting up the pinned TypeScript compiler (first run only)");
    install(path, &source)?;
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
