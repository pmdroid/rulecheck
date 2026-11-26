use crate::policy::{CompiledRule, Location, MAX_FILE, Result, bounded_read, valid_path};
use rustix::fs::{Mode, OFlags, open, openat};
use rustix::io::Errno;
use std::fs::File;
use std::io::BufReader;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_INDEX: usize = 8 * 1024 * 1024;
const MAX_TEXT: usize = 64 * 1024 * 1024;
const MAX_FINDINGS: usize = 10_000;

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Finding<'a> {
    path: &'a str,
    location: Location,
    id: &'a str,
    remediation: &'a str,
}

pub fn check(root: &Path, rules: &[&CompiledRule]) -> Result<bool> {
    let directory = open(root, directory_flags(), Mode::empty())
        .map_err(|err| format!("cannot open checkout {}: {err}", root.display()))?;
    if !git_output(root, &["rev-parse", "--show-prefix"], 4096)?
        .iter()
        .all(u8::is_ascii_whitespace)
    {
        return Err("select the root directory of a Git checkout with --root".into());
    }
    let index = git_output(root, &["ls-files", "--stage", "-z"], MAX_INDEX)?;
    let paths = tracked_regular_paths(&index)?;
    let mut hits = vec![0; rules.len()];
    let mut findings = Vec::new();
    let mut text_bytes = 0;
    for path in paths {
        let mut applicable = rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.applies_to(path));
        let Some((_, first)) = applicable.clone().next() else {
            continue;
        };
        let context = |err| format!("[{}] {path}: {err}", first.id);
        let Some(file) = open_candidate(&directory, path).map_err(context)? else {
            continue;
        };

        let content = if let Some((_, text_rule)) = applicable
            .clone()
            .find(|(_, rule)| rule.matcher.needs_text())
        {
            let context = |err| format!("[{}] {path}: {err}", text_rule.id);
            let bytes = bounded_read(BufReader::new(file), MAX_FILE)
                .map_err(|err| context(format!("cannot read text: {err}")))?;
            text_bytes += bytes.len();
            if text_bytes > MAX_TEXT {
                return Err(context(
                    "scan exceeds 64 MiB of text; narrow the path scope".into(),
                ));
            }
            Some(String::from_utf8(bytes).map_err(|_| {
                context("expected UTF-8 text; fix the file or narrow the path scope".into())
            })?)
        } else {
            None
        };

        for (index, rule) in &mut applicable {
            hits[index] += 1;
            if let Some(location) = rule.matcher.location(content.as_deref()) {
                if findings.len() >= MAX_FINDINGS {
                    return Err(context(
                        "more than 10,000 findings; narrow the path scope".into(),
                    ));
                }
                findings.push(Finding {
                    path,
                    location,
                    id: &rule.id,
                    remediation: &rule.remediation,
                });
            }
        }
    }

    let mut empty_scope = false;
    for (rule, count) in rules.iter().zip(hits) {
        if rule.matcher.needs_text() && count == 0 {
            eprintln!(
                "[{}] empty text scope: track a matching file or update the rule's path globs",
                rule.id
            );
            empty_scope = true;
        }
    }
    findings.sort_unstable();
    for finding in &findings {
        let Location { line, column } = finding.location;
        println!(
            "{}:{line}:{column}: [{}] {}",
            finding.path, finding.id, finding.remediation
        );
    }
    if empty_scope {
        return Err("one or more text rules matched no repository files".into());
    }
    Ok(findings.is_empty())
}

fn git_output(root: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot start Git: {err}"))?;
    let output = bounded_read(child.stdout.take().expect("piped stdout"), limit);
    if output.is_err() {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|err| format!("cannot wait for Git: {err}"))?;
    let bytes = output.map_err(|err| format!("cannot read Git checkout: {err}"))?;
    if !status.success() {
        return Err("Git could not inspect the checkout".into());
    }
    Ok(bytes)
}

fn tracked_regular_paths(index: &[u8]) -> Result<Vec<&str>> {
    let mut paths = Vec::new();
    let index = std::str::from_utf8(index).map_err(|_| "Git paths must be UTF-8")?;
    for record in index.split_terminator('\0') {
        let (fields, path) = record.split_once('\t').ok_or("invalid Git index entry")?;
        let (mode, rest) = fields.split_once(' ').ok_or("invalid Git index mode")?;
        if !rest.ends_with(" 0") {
            return Err(format!(
                "unmerged Git index entry {path:?}; resolve conflicts before checking"
            ));
        }
        if matches!(mode, "100644" | "100755") {
            if !valid_path(path) {
                return Err(format!("unsupported Git path {path:?}"));
            }
            paths.push(path);
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn open_candidate(root: &OwnedFd, path: &str) -> Result<Option<File>> {
    let mut segments = path.split('/').peekable();
    let mut parent = None;
    while let Some(segment) = segments.next() {
        let directory = parent.as_ref().unwrap_or(root);
        let last = segments.peek().is_none();
        let flags = if last {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
        } else {
            directory_flags()
        };
        let fd = match openat(directory, segment, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(err) => {
                return Err(format!(
                    "cannot open {segment:?} without following symlinks: {err}"
                ));
            }
        };
        if last {
            let file = File::from(fd);
            let metadata = file
                .metadata()
                .map_err(|err| format!("cannot inspect file: {err}"))?;
            if !metadata.is_file() {
                return Err("expected a regular file".into());
            }
            return Ok(Some(file));
        }
        parent = Some(fd);
    }
    Err("empty repository path".into())
}
