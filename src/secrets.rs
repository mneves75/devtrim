//! Read-only search for credential shapes in coding-agent transcripts.
//!
//! Claude Code, Codex and Gemini CLI keep conversation logs on disk. A key
//! pasted into a chat or printed by a command stays in those logs. This scan
//! reads the known transcript locations and reports a mask: the public prefix
//! and the last four characters. The secret itself is never printed, and no
//! transcript is modified.
//!
//! Token shapes follow the publishers' documented prefixes (GitHub token
//! formats, AWS access key IDs, and the `sk-` / `sk-ant-` key prefixes). A
//! string of one repeated character is not reported.

use anyhow::Result;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::report::{Action, Finding};
use crate::safety::{self, Ctx};

const MAX_LINE: usize = 1024 * 1024;
const MAX_MATCHES_PER_FILE: usize = 32;

/// Transcript roots relative to a home directory, and which agent wrote them.
const HOME_SOURCES: &[(&str, &str)] = &[
    ("Claude Code", ".claude/projects"),
    ("Claude Code", ".claude/history.jsonl"),
    ("Codex", ".codex/sessions"),
    ("Codex", ".codex/archived_sessions"),
    ("Codex", ".codex/history.jsonl"),
    ("Gemini CLI", ".gemini/tmp"),
];

const CODEX_RELATIVE: &[&str] = &["sessions", "archived_sessions", "history.jsonl"];

struct Pattern {
    kind: &'static str,
    prefix: &'static str,
    min_body: usize,
    body: fn(u8) -> bool,
}

fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn upper_or_digit(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}

fn alnum(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

/// Longer prefixes come first so `sk-ant-` is not also reported as `sk-`.
const PATTERNS: &[Pattern] = &[
    Pattern {
        kind: "Anthropic API key",
        prefix: "sk-ant-",
        min_body: 32,
        body: token_char,
    },
    Pattern {
        kind: "OpenAI API key",
        prefix: "sk-proj-",
        min_body: 32,
        body: token_char,
    },
    Pattern {
        kind: "OpenAI API key",
        prefix: "sk-",
        min_body: 40,
        body: token_char,
    },
    Pattern {
        kind: "Stripe secret key",
        prefix: "sk_live_",
        min_body: 20,
        body: alnum,
    },
    Pattern {
        kind: "GitHub token",
        prefix: "github_pat_",
        min_body: 40,
        body: token_char,
    },
    Pattern {
        kind: "GitHub token",
        prefix: "ghp_",
        min_body: 36,
        body: alnum,
    },
    Pattern {
        kind: "GitHub token",
        prefix: "gho_",
        min_body: 36,
        body: alnum,
    },
    Pattern {
        kind: "GitHub token",
        prefix: "ghs_",
        min_body: 36,
        body: alnum,
    },
    Pattern {
        kind: "GitHub token",
        prefix: "ghu_",
        min_body: 36,
        body: alnum,
    },
    Pattern {
        kind: "AWS access key",
        prefix: "AKIA",
        min_body: 16,
        body: upper_or_digit,
    },
    Pattern {
        kind: "Google API key",
        prefix: "AIza",
        min_body: 35,
        body: token_char,
    },
    Pattern {
        kind: "Slack token",
        prefix: "xoxb-",
        min_body: 20,
        body: token_char,
    },
    Pattern {
        kind: "Slack token",
        prefix: "xoxp-",
        min_body: 20,
        body: token_char,
    },
    Pattern {
        kind: "Hugging Face token",
        prefix: "hf_",
        min_body: 30,
        body: alnum,
    },
    Pattern {
        kind: "npm token",
        prefix: "npm_",
        min_body: 36,
        body: alnum,
    },
    Pattern {
        kind: "Private key",
        prefix: "-----BEGIN ",
        min_body: 0,
        body: |_| false,
    },
];

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Masked {
    kind: &'static str,
    masked: String,
}

struct Hit {
    masked: Masked,
    line: u64,
    occurrences: u64,
}

pub fn scan(home: &Path) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    let mut sources = HOME_SOURCES
        .iter()
        .map(|(agent, relative)| (*agent, home.join(relative)))
        .collect::<Vec<_>>();
    for codex_home in safety::configured_codex_homes(home)? {
        for relative in CODEX_RELATIVE {
            sources.push(("Codex", codex_home.join(relative)));
        }
    }
    for (agent, root) in sources {
        scan_source(agent, &root, &mut findings);
    }
    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.label.cmp(&right.label))
    });
    Ok(findings)
}

pub fn run(ctx: &Ctx) -> Result<std::process::ExitCode> {
    let findings = scan(&ctx.home)?;
    let errors: Vec<String> = findings
        .iter()
        .filter_map(Finding::scan_error)
        .map(str::to_owned)
        .collect();
    if ctx.json {
        crate::report::print_json("secrets", false, &findings, None, &errors)?;
    } else if findings.is_empty() {
        crate::report::print_line("no credential patterns in coding-agent transcripts")?;
    } else {
        crate::report::print_human(&findings)?;
        for error in &errors {
            ctx.diagnostic("warn", error.clone());
        }
    }
    Ok(if errors.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::from(1)
    })
}

fn scan_source(agent: &str, root: &Path, findings: &mut Vec<Finding>) {
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            findings.push(error_finding(
                agent,
                root,
                format!("cannot inspect {}: {error}", root.display()),
            ));
            return;
        }
    };
    if metadata.file_type().is_symlink() {
        findings.push(error_finding(
            agent,
            root,
            format!(
                "refusing to follow a transcript path that is a symlink: {}",
                root.display()
            ),
        ));
        return;
    }
    if metadata.is_file() {
        if is_transcript(root) {
            scan_transcript(agent, root, findings);
        }
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let device = metadata.dev();
    let mut stack = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                findings.push(error_finding(
                    agent,
                    &dir,
                    format!("cannot read {}: {error}", dir.display()),
                ));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    findings.push(error_finding(
                        agent,
                        &dir,
                        format!("cannot read {}: {error}", dir.display()),
                    ));
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    findings.push(error_finding(
                        agent,
                        &path,
                        format!("cannot inspect {}: {error}", path.display()),
                    ));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || metadata.dev() != device {
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() && is_transcript(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        scan_transcript(agent, &path, findings);
    }
}

fn open_nofollow(path: &Path) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags};

    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(File::from(descriptor))
}

fn is_transcript(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("json") || extension.eq_ignore_ascii_case("jsonl")
        })
}

fn scan_transcript(agent: &str, path: &Path, findings: &mut Vec<Finding>) {
    let file = match open_nofollow(path) {
        Ok(file) => file,
        Err(error) => {
            findings.push(error_finding(
                agent,
                path,
                format!("cannot read {}: {error}", path.display()),
            ));
            return;
        }
    };
    // Opened with `O_NOFOLLOW`, so a symlink that replaced the file after the
    // type check is an error instead of a read of the link target. The file
    // is never written.
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut found: BTreeMap<Masked, Hit> = BTreeMap::new();
    let mut omitted = 0u64;
    let mut buffer = Vec::new();
    let mut line_no = 0u64;
    loop {
        buffer.clear();
        match reader
            .by_ref()
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut buffer)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buffer.last() != Some(&b'\n') && buffer.len() == MAX_LINE {
            skip_rest_of_line(&mut reader);
        }
        line_no += 1;
        for masked in scan_line(&buffer) {
            if let Some(hit) = found.get_mut(&masked) {
                hit.occurrences = hit.occurrences.saturating_add(1);
                continue;
            }
            if found.len() >= MAX_MATCHES_PER_FILE {
                omitted = omitted.saturating_add(1);
                continue;
            }
            found.insert(
                masked.clone(),
                Hit {
                    masked,
                    line: line_no,
                    occurrences: 1,
                },
            );
        }
    }
    let mut hits = found.into_values().collect::<Vec<_>>();
    hits.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then(left.masked.cmp(&right.masked))
    });
    for hit in hits {
        let times = if hit.occurrences == 1 {
            "once".to_string()
        } else {
            format!("{} times", hit.occurrences)
        };
        findings.push(Finding::new(
            format!("{agent} transcript: {}", hit.masked.kind),
            Some(path.to_path_buf()),
            0,
            format!("line {}, {times}; masked {}", hit.line, hit.masked.masked),
            4,
            Action::Info,
        ));
    }
    if omitted > 0 {
        findings.push(error_finding(
            agent,
            path,
            format!(
                "{omitted} further credential patterns in {} were omitted after {MAX_MATCHES_PER_FILE}",
                path.display()
            ),
        ));
    }
}

fn scan_line(line: &[u8]) -> Vec<Masked> {
    let mut found = Vec::new();
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for pattern in PATTERNS {
        let prefix = pattern.prefix.as_bytes();
        let mut from = 0;
        while let Some(offset) = find_bytes(&line[from..], prefix) {
            let start = from + offset;
            from = start + prefix.len();
            if start > 0 && token_char(line[start - 1]) {
                continue;
            }
            if taken
                .iter()
                .any(|&(begin, end)| start >= begin && start < end)
            {
                continue;
            }
            if pattern.kind == "Private key" {
                let rest = &line[from..line.len().min(from + 40)];
                if find_bytes(rest, b"PRIVATE KEY-----").is_some() {
                    taken.push((start, from));
                    found.push(Masked {
                        kind: pattern.kind,
                        masked: "-----BEGIN … PRIVATE KEY-----".to_string(),
                    });
                }
                continue;
            }
            let length = line[from..]
                .iter()
                .take_while(|byte| (pattern.body)(**byte))
                .count();
            let body = &line[from..from + length];
            if length < pattern.min_body || !looks_real(body) {
                continue;
            }
            taken.push((start, from + length));
            found.push(Masked {
                kind: pattern.kind,
                masked: mask(pattern.prefix, body),
            });
        }
    }
    found
}

fn looks_real(body: &[u8]) -> bool {
    let digits = body.iter().filter(|byte| byte.is_ascii_digit()).count();
    let letters = body
        .iter()
        .filter(|byte| byte.is_ascii_alphabetic())
        .count();
    let mut seen = [false; 256];
    let distinct = body
        .iter()
        .filter(|byte| {
            let index = usize::from(**byte);
            let fresh = !seen[index];
            seen[index] = true;
            fresh
        })
        .count();
    digits >= 2 && letters >= 4 && distinct >= 10
}

fn mask(prefix: &str, body: &[u8]) -> String {
    let tail = body.len().saturating_sub(4);
    let last = String::from_utf8_lossy(&body[tail..]);
    format!("{prefix}…{last}")
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn skip_rest_of_line(reader: &mut impl BufRead) {
    loop {
        let (done, used) = match reader.fill_buf() {
            Ok([]) | Err(_) => return,
            Ok(chunk) => match chunk.iter().position(|byte| *byte == b'\n') {
                Some(index) => (true, index + 1),
                None => (false, chunk.len()),
            },
        };
        reader.consume(used);
        if done {
            return;
        }
    }
}

fn error_finding(agent: &str, path: &Path, message: String) -> Finding {
    Finding::new(
        format!("{agent} transcript: unreadable"),
        Some(path.to_path_buf()),
        0,
        message.clone(),
        1,
        Action::Info,
    )
    .with_scan_error(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(seed: &str, len: usize) -> String {
        let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut out = String::new();
        let seed = seed.as_bytes();
        while out.len() < len {
            let byte =
                alphabet[(seed[out.len() % seed.len()] as usize + out.len()) % alphabet.len()];
            out.push(char::from(byte));
        }
        out
    }

    #[test]
    fn masks_a_real_key_and_ignores_a_placeholder() {
        let real = format!("sk-ant-{}", body("anthropic-key", 40));
        let line = format!("pasted {real} and sk-ant-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        let hits = scan_line(line.as_bytes());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "Anthropic API key");
        assert!(hits[0].masked.starts_with("sk-ant-…"));
        assert!(!hits[0].masked.contains(&real[8..20]));
        assert!(!line.contains(&hits[0].masked.replace('…', "")));
        assert_eq!(scan_line(b"AKIAEXAMPLEEXAMPLE").len(), 0);
    }

    #[test]
    fn a_more_specific_prefix_is_not_reported_twice() {
        let key = format!("sk-ant-{}", body("specific-prefix", 40));
        let hits = scan_line(key.as_bytes());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "Anthropic API key");
    }

    #[test]
    fn private_key_material_is_not_part_of_the_mask() {
        // Built at runtime so the source does not contain a PEM banner.
        let header = format!("-----{} {} {} KEY-----", "BEGIN", "OPENSSH", "PRIVATE");
        let footer = format!("-----{} {} {} KEY-----", "END", "OPENSSH", "PRIVATE");
        let material = "not-a-key-body";
        let line = format!("{header}\n{material}\n{footer}");
        let hits = scan_line(line.as_bytes());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].masked, "-----BEGIN … PRIVATE KEY-----");
        assert!(!hits[0].masked.contains(material));
    }

    #[test]
    fn scan_reports_a_transcript_and_does_not_follow_a_symlink() {
        let fixture = crate::ops::TestFixture::new("devtrim-secrets-scan");
        let home = fixture.path();
        let key = format!("ghp_{}", body("github-personal", 36));
        let transcript = home.join(".claude/projects/demo/session.jsonl");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, format!("user said {key}\n")).unwrap();
        let outside = home.join("outside/secret.jsonl");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        let outside_key = format!("sk-ant-{}", body("outside-secret", 40));
        std::fs::write(&outside, format!("{outside_key}\n")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join(".claude/projects/demo/linked.jsonl"))
            .unwrap();
        std::fs::write(
            home.join(".claude/projects/demo/notes.md"),
            format!("{key}\n"),
        )
        .unwrap();

        let findings = scan(home).unwrap();
        let rendered = findings
            .iter()
            .map(|finding| format!("{} {}", finding.label, finding.note))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Claude Code transcript: GitHub token"));
        assert!(rendered.contains("ghp_…"));
        assert!(!rendered.contains(&key));
        assert!(!rendered.contains(&outside_key));
        assert!(!rendered.contains("AAAABODY"));
        assert_eq!(
            std::fs::read_to_string(&transcript).unwrap(),
            format!("user said {key}\n")
        );
    }

    #[test]
    fn an_unreadable_transcript_is_disclosed_and_the_other_is_still_reported() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = crate::ops::TestFixture::new("devtrim-secrets-unreadable");
        let home = fixture.path();
        let key = format!("npm_{}", body("npm-token-value", 36));
        let readable = home.join(".gemini/tmp/demo/chat.json");
        std::fs::create_dir_all(readable.parent().unwrap()).unwrap();
        std::fs::write(&readable, format!("{key}\n")).unwrap();
        let hidden = home.join(".codex/sessions/day/session.jsonl");
        std::fs::create_dir_all(hidden.parent().unwrap()).unwrap();
        std::fs::write(&hidden, format!("{key}\n")).unwrap();
        let mut permissions = std::fs::metadata(&hidden).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&hidden, permissions).unwrap();

        let findings = scan(home).unwrap();
        let rendered = findings
            .iter()
            .map(|finding| format!("{} {}", finding.label, finding.note))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Gemini CLI transcript"));
        assert!(
            findings
                .iter()
                .any(|finding| finding.scan_error().is_some())
        );
        assert!(!rendered.contains(&key));
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}
