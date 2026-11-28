use crate::policy::{CompiledRule, Location, MAX_FILE, Result, Span, bounded_read, valid_path};
use rustix::fs::{Mode, OFlags, open, openat};
use rustix::io::Errno;
use std::fs::File;
use std::io::BufReader;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_ENTRIES: usize = 8 * 1024 * 1024;
/// Default and largest `--text-budget`, in MiB, for the scoped text one scan reads.
pub const DEFAULT_TEXT_BUDGET_MIB: usize = 64;
pub const MAX_TEXT_BUDGET_MIB: usize = 1024;
const MAX_FINDINGS: usize = 10_000;

/// One rule's first match in one file, or with all-match mode one of its
/// matches. The derived order (path, location, rule ID, then byte range) is the
/// published output order.
#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub struct Finding<'a> {
    pub path: String,
    pub location: Location,
    pub id: &'a str,
    pub span: Option<Span>,
    pub remediation: &'a str,
}

/// How many findings each rule reports per file.
#[derive(Clone, Copy, PartialEq)]
pub enum Matches {
    /// The v1 default: the first match.
    First,
    /// Every match of a text rule; still one finding per forbidden path.
    All,
}

/// A finished or budget-stopped scan. Text rules in `empty_scopes` matched no
/// candidate file, so the scan cannot judge them and the command must exit 2.
/// With `partial` set the scan stopped at the text budget: `findings` are only
/// those from the files before the stop, and the command must exit 2.
pub struct Report<'a> {
    pub findings: Vec<Finding<'a>>,
    pub empty_scopes: Vec<&'a str>,
    pub partial: Option<Partial<'a>>,
}

/// Where a scan stopped because the next scoped text file would exceed the
/// text budget. Every rule that applies to that file or a later candidate is in
/// `unevaluated`, in policy order; the other rules were evaluated on all files.
pub struct Partial<'a> {
    pub budget: usize,
    /// Scoped text read and evaluated before the stop.
    pub read: usize,
    /// The file that did not fit, and the total it would have brought.
    pub path: String,
    pub needed: usize,
    pub unevaluated: Vec<&'a str>,
}

pub const ROOT_HINT: &str = "run from the checkout root or pass --root";

/// Scans the working tree. The inventory is the regular files in the Git index;
/// with `untracked`, non-ignored untracked files are added. Both are read with
/// the same no-follow rules, and deleted index entries are skipped.
pub fn check<'a>(
    root: &Path,
    rules: &[&'a CompiledRule],
    untracked: bool,
    matches: Matches,
    budget: usize,
) -> Result<Report<'a>> {
    let directory = open(root, directory_flags(), Mode::empty())
        .map_err(|err| format!("cannot open checkout {}: {err}", root.display()))?;
    if !git_output(root, &["rev-parse", "--show-prefix"], 4096)?
        .iter()
        .all(u8::is_ascii_whitespace)
    {
        return Err(format!(
            "{} is a subdirectory of a Git checkout, not its root; {ROOT_HINT}",
            root.display()
        ));
    }
    let index = git_output(root, &["ls-files", "--stage", "-z"], MAX_ENTRIES)?;
    let mut paths = tracked_regular_paths(&index)?;
    let others;
    if untracked {
        others = git_output(
            root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            MAX_ENTRIES,
        )?;
        paths.extend(untracked_paths(&others)?);
        paths.sort_unstable();
        paths.dedup();
    }
    scan(paths, rules, matches, budget, |path, needs_text| {
        let Some(file) = open_candidate(&directory, path)? else {
            return Ok(None);
        };
        let bytes = if needs_text {
            bounded_read(BufReader::new(file), MAX_FILE)
                .map_err(|err| format!("cannot read text: {err}"))?
        } else {
            Vec::new()
        };
        Ok(Some(bytes))
    })
}

pub fn check_commit<'a>(
    root: &Path,
    sha: &str,
    rules: &[&'a CompiledRule],
    matches: Matches,
    budget: usize,
) -> Result<Report<'a>> {
    if git_output(root, &["cat-file", "-t", sha], 32)? != b"commit\n" {
        return Err("--commit requires a Git commit object".into());
    }
    let tree = git_output(
        root,
        &["ls-tree", "--full-tree", "-r", "-z", sha],
        MAX_ENTRIES,
    )?;
    let tree = std::str::from_utf8(&tree).map_err(|_| "Git paths must be UTF-8")?;
    let mut entries = std::collections::BTreeMap::new();
    for record in tree.split_terminator('\0') {
        let (fields, path) = record.split_once('\t').ok_or("invalid Git tree entry")?;
        let (kind, oid) = fields.rsplit_once(' ').ok_or("invalid Git tree entry")?;
        if !valid_path(path) || entries.insert(path, (kind, oid)).is_some() {
            return Err(format!("unsupported Git path {path:?}"));
        }
    }
    scan(
        entries.keys().copied(),
        rules,
        matches,
        budget,
        |path, needs_text| {
            let (kind, oid) = entries[path];
            if !matches!(kind, "100644 blob" | "100755 blob") {
                return Err("expected a regular Git blob for this scoped path".into());
            }
            if git_output(root, &["cat-file", "-t", oid], 32)? != b"blob\n" {
                return Err("expected a Git blob object".into());
            }
            let bytes = if needs_text {
                git_output(root, &["cat-file", "blob", oid], MAX_FILE)?
            } else {
                Vec::new()
            };
            Ok(Some(bytes))
        },
    )
}

fn scan<'a, 'p>(
    paths: impl IntoIterator<Item = &'p str>,
    rules: &[&'a CompiledRule],
    matches: Matches,
    budget: usize,
    read: impl Fn(&str, bool) -> Result<Option<Vec<u8>>>,
) -> Result<Report<'a>> {
    let mut hits = vec![0; rules.len()];
    let mut findings = Vec::new();
    let mut text_bytes = 0;
    let mut partial: Option<Partial> = None;
    let mut unevaluated = vec![false; rules.len()];
    for path in paths {
        let mut applicable = rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.applies_to(path));
        let Some((_, first)) = applicable.clone().next() else {
            continue;
        };
        // After a budget stop nothing more is read; the remaining candidates
        // only mark which rules were not evaluated on every file in scope.
        if partial.is_some() {
            applicable.for_each(|(index, _)| unevaluated[index] = true);
            continue;
        }
        let context = |err| format!("[{}] {path}: {err}", first.id);
        let text_rule = applicable
            .clone()
            .find(|(_, rule)| rule.matcher.needs_text());
        let read_context = |err| {
            format!(
                "[{}] {path}: {err}",
                text_rule.map_or(first, |(_, rule)| rule).id
            )
        };
        let Some(bytes) = read(path, text_rule.is_some()).map_err(read_context)? else {
            continue;
        };

        let content = if text_rule.is_some() {
            // Each file is read once for all its rules and counted once.
            if text_bytes + bytes.len() > budget {
                partial = Some(Partial {
                    budget,
                    read: text_bytes,
                    path: path.to_owned(),
                    needed: text_bytes + bytes.len(),
                    unevaluated: Vec::new(),
                });
                applicable.for_each(|(index, _)| unevaluated[index] = true);
                continue;
            }
            text_bytes += bytes.len();
            Some(String::from_utf8(bytes).map_err(|_| {
                read_context("expected UTF-8 text; fix the file or narrow the path scope".into())
            })?)
        } else {
            None
        };

        for (index, rule) in &mut applicable {
            hits[index] += 1;
            let found = match matches {
                Matches::First => rule
                    .matcher
                    .find(path, content.as_deref())
                    .map(Vec::from_iter),
                // One more than fits, so an overflow is detected without
                // collecting every match of a huge file.
                Matches::All => rule.matcher.find_all(
                    path,
                    content.as_deref(),
                    MAX_FINDINGS - findings.len() + 1,
                ),
            }
            .map_err(|err| format!("[{}] {path}: {err}", rule.id))?;
            for (location, span) in found {
                if findings.len() >= MAX_FINDINGS {
                    return Err(context("more than 10,000 findings; narrow the path scope"));
                }
                findings.push(Finding {
                    path: path.to_owned(),
                    location,
                    id: &rule.id,
                    remediation: &rule.remediation,
                    span,
                });
            }
        }
    }

    let empty_scopes = rules
        .iter()
        .zip(hits)
        .zip(&unevaluated)
        .filter(|((rule, count), skipped)| rule.matcher.needs_text() && *count == 0 && !**skipped)
        .map(|((rule, _), _)| rule.id.as_str())
        .collect();
    if let Some(partial) = &mut partial {
        partial.unevaluated = rules
            .iter()
            .zip(&unevaluated)
            .filter(|(_, skipped)| **skipped)
            .map(|(rule, _)| rule.id.as_str())
            .collect();
    }
    findings.sort_unstable();
    Ok(Report {
        findings,
        empty_scopes,
        partial,
    })
}

fn git_output(root: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>> {
    let mut child = Command::new("git")
        .arg("--no-replace-objects")
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

/// Parses `git ls-files --others -z`. Git lists untracked regular files and
/// symlinks (FIFOs, sockets and devices are invisible to it) and shows a nested
/// repository as one `dir/` entry, which is skipped like a gitlink in the index.
fn untracked_paths(others: &[u8]) -> Result<Vec<&str>> {
    let others = std::str::from_utf8(others).map_err(|_| "Git paths must be UTF-8")?;
    let mut paths = Vec::new();
    for path in others.split_terminator('\0') {
        if path.ends_with('/') {
            continue;
        }
        if !valid_path(path) {
            return Err(format!("unsupported Git path {path:?}"));
        }
        paths.push(path);
    }
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
