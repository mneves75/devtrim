//! Docker/OrbStack pruning: unused images + build cache. Volumes are never pruned.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use super::{Action, ApplyOutcome, Finding, Op, command_stdout, optional_command_stdout};
use crate::process::{BoundedCommand as _, QUERY_TIMEOUT};
use crate::report::{BuildxNodeIdentity, CommandAuthority};
use crate::safety::{Ctx, escalate};

pub(crate) struct Docker;

/// Host-side virtual disk images for the supported local Docker runtimes,
/// relative to the user's home directory.
///
/// `docker system df` reports space *inside* the guest filesystem. The host
/// pays for these files instead, and a prune inside the VM shrinks them only
/// when the runtime hands the freed blocks back. OrbStack documents that as
/// automatic (<https://docs.orbstack.dev/faq>) and Docker Desktop as taking a
/// few seconds, but it is not guaranteed (orbstack/orbstack#2030), so the host
/// file is measured rather than assumed. Reporting only the guest number
/// understates the host cost, so devtrim discloses the image itself.
const VM_DISK_IMAGES: &[(&str, &str)] = &[
    (
        "OrbStack",
        "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img.raw",
    ),
    (
        "Docker Desktop",
        "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
    ),
    (
        "Docker Desktop",
        "Library/Containers/com.docker.docker/Data/vms/0/Docker.raw",
    ),
];

#[derive(serde::Deserialize)]
struct DockerContext {
    #[serde(rename = "Endpoints")]
    endpoints: DockerEndpoints,
}

#[derive(serde::Deserialize)]
struct DockerEndpoints {
    docker: DockerEndpoint,
}

#[derive(serde::Deserialize)]
struct DockerEndpoint {
    #[serde(rename = "Host")]
    host: String,
}

#[derive(serde::Deserialize)]
struct DockerContextListRow {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "DockerEndpoint", default)]
    docker_endpoint: String,
}

// Go's `encoding/json` marshals nil slices as `null`, so `nodes` and `ids`
// use `Option<Vec<...>>` with `#[serde(default)]`.
#[derive(Debug, Clone, serde::Deserialize)]
struct BuildxBuilder {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Driver", default)]
    driver: String,
    #[serde(rename = "Current", default)]
    current: bool,
    #[serde(rename = "Nodes", default)]
    nodes: Option<Vec<BuildxNode>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct BuildxNode {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Endpoint", default)]
    endpoint: String,
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "IDs", default)]
    ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildxUsage {
    total_bytes: u64,
    total_text: String,
    reclaimable_bytes: u64,
}

fn docker(home: &Path, docker_config: &Path, host: &str, args: &[&str]) -> Result<String> {
    let command = format!("`docker --host {host} {}`", args.join(" "));
    let mut process = Command::new("docker");
    process
        .current_dir(home)
        .env("DOCKER_CONFIG", docker_config)
        .arg("--host")
        .arg(host)
        .args(args);
    if matches!(args.first().copied(), Some("version" | "system")) {
        process
            .env_remove("DOCKER_CONTEXT")
            .env_remove("BUILDX_BUILDER")
            .env_remove("BUILDX_CONFIG");
    } else if matches!(args.first().copied(), Some("buildx")) {
        process
            .env("BUILDX_CONFIG", docker_config.join("buildx"))
            .env_remove("BUILDX_BUILDER");
    }
    command_stdout(process.output_within(QUERY_TIMEOUT), &command)
}

fn docker_host(home: &Path, docker_config: &Path) -> Result<Option<String>> {
    let Some(output) = optional_command_stdout(
        Command::new("docker")
            .current_dir(home)
            .env("DOCKER_CONFIG", docker_config)
            .args(["context", "inspect"])
            .output_within(QUERY_TIMEOUT),
        "`docker context inspect`",
    )?
    else {
        return Ok(None);
    };
    parse_docker_host(&output).map(Some)
}

fn parse_docker_host(output: &str) -> Result<String> {
    let contexts: Vec<DockerContext> =
        serde_json::from_str(output).context("docker context inspect returned invalid JSON")?;
    let [context] = contexts.as_slice() else {
        anyhow::bail!("docker context inspect must return exactly one active context");
    };
    let host = context.endpoints.docker.host.trim();
    if !is_local_docker_host(host) {
        anyhow::bail!("refusing non-local Docker endpoint `{host}`");
    }
    Ok(host.to_string())
}

fn is_local_docker_host(host: &str) -> bool {
    let Some(socket) = host.strip_prefix("unix://") else {
        return false;
    };
    let path = Path::new(socket);
    path.is_absolute()
        && path.file_name().is_some()
        && path
            .components()
            .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
}

fn is_valid_builder_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    bytes.len() <= 128
        && first.is_ascii_alphanumeric()
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn local_docker_contexts(home: &Path, docker_config: &Path) -> Result<BTreeMap<String, String>> {
    let output = command_stdout(
        Command::new("docker")
            .current_dir(home)
            .env("DOCKER_CONFIG", docker_config)
            .args(["context", "ls", "--format", "{{json .}}"])
            .output_within(QUERY_TIMEOUT),
        "`docker context ls --format '{{json .}}'`",
    )?;
    parse_local_docker_contexts(&output)
}

fn parse_local_docker_contexts(output: &str) -> Result<BTreeMap<String, String>> {
    let mut contexts = BTreeMap::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let row: DockerContextListRow =
            serde_json::from_str(trimmed).context("docker context ls returned invalid JSON")?;
        let name = row.name.trim();
        let endpoint = row.docker_endpoint.trim();
        if !name.is_empty() && is_local_docker_host(endpoint) {
            contexts.insert(name.to_string(), endpoint.to_string());
        }
    }
    Ok(contexts)
}

fn is_buildx_unavailable(stderr: &str) -> bool {
    let lowered = stderr.to_ascii_lowercase();
    lowered.contains("is not a docker command")
        || lowered.contains("unknown command")
        || lowered.contains("unknown shorthand flag")
}

fn buildx_ls(home: &Path, docker_config: &Path, host: &str) -> Result<Option<String>> {
    let output = Command::new("docker")
        .current_dir(home)
        .env("DOCKER_CONFIG", docker_config)
        .env("BUILDX_CONFIG", docker_config.join("buildx"))
        .env_remove("BUILDX_BUILDER")
        .arg("--host")
        .arg(host)
        .args(["buildx", "ls", "--format", "{{json .}}"])
        .output_within(QUERY_TIMEOUT);
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("cannot run `docker --host {host} buildx ls --format '{{{{json .}}}}'`")
            });
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_buildx_unavailable(&stderr) {
            return Ok(None);
        }
        let detail = stderr.trim();
        if detail.is_empty() {
            anyhow::bail!(
                "`docker --host {host} buildx ls --format '{{{{json .}}}}'` failed with {}",
                output.status
            );
        }
        anyhow::bail!(
            "`docker --host {host} buildx ls --format '{{{{json .}}}}'` failed with {}: {detail}",
            output.status
        );
    }
    String::from_utf8(output.stdout).map(Some).with_context(|| {
        format!(
            "`docker --host {host} buildx ls --format '{{{{json .}}}}'` returned non-UTF-8 output"
        )
    })
}

/// Parses NDJSON from `docker buildx ls --format '{{json .}}'`, collapsing
/// duplicate rows emitted per builder/node by builder name.
fn parse_buildx_ls(output: &str) -> Result<Vec<BuildxBuilder>> {
    let mut by_name: BTreeMap<String, BuildxBuilder> = BTreeMap::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: BuildxBuilder =
            serde_json::from_str(trimmed).context("docker buildx ls returned invalid JSON")?;
        match by_name.get_mut(&parsed.name) {
            Some(existing) => {
                existing.current |= parsed.current;
                if existing.driver.trim().is_empty() && !parsed.driver.trim().is_empty() {
                    existing.driver = parsed.driver;
                }
                let existing_empty = existing.nodes.as_ref().is_none_or(Vec::is_empty);
                let parsed_non_empty = parsed.nodes.as_ref().is_some_and(|nodes| !nodes.is_empty());
                if existing_empty && parsed_non_empty {
                    existing.nodes = parsed.nodes;
                }
            }
            None => {
                by_name.insert(parsed.name.clone(), parsed);
            }
        }
    }
    Ok(by_name.into_values().collect())
}

fn is_local_buildx_node(node: &BuildxNode, local_contexts: &BTreeMap<String, String>) -> bool {
    if node.status.trim() != "running" {
        return false;
    }
    let has_worker_id = node
        .ids
        .as_deref()
        .is_some_and(|ids| ids.iter().any(|identifier| !identifier.trim().is_empty()));
    if !has_worker_id {
        return false;
    }
    let endpoint = node.endpoint.trim();
    is_local_docker_host(endpoint) || local_contexts.contains_key(endpoint)
}

fn needs_context_lookup(builders: &[BuildxBuilder]) -> bool {
    builders.iter().any(|builder| {
        builder.driver.trim() == "docker-container"
            && is_valid_builder_name(&builder.name)
            && builder.nodes.as_deref().is_some_and(|nodes| {
                !nodes.is_empty()
                    && nodes.iter().any(|node| {
                        let endpoint = node.endpoint.trim();
                        !is_local_docker_host(endpoint) && !endpoint.contains("://")
                    })
            })
    })
}

/// Canonical per-node identity combining each node's name, resolved local
/// socket endpoint, and sorted worker IDs structurally so neither an un-ID'd
/// node, a delimiter character, nor a retargeted context alias can slip past
/// apply re-verification.
fn builder_node_fingerprint(
    nodes: &[BuildxNode],
    local_contexts: &BTreeMap<String, String>,
) -> Vec<BuildxNodeIdentity> {
    let mut entries: Vec<BuildxNodeIdentity> = nodes
        .iter()
        .map(|node| {
            let endpoint = node.endpoint.trim();
            let resolved = local_contexts
                .get(endpoint)
                .map(String::as_str)
                .unwrap_or(endpoint);
            let mut ids: Vec<String> = node
                .ids
                .iter()
                .flatten()
                .map(|identifier| identifier.trim().to_string())
                .filter(|identifier| !identifier.is_empty())
                .collect();
            ids.sort();
            ids.dedup();
            BuildxNodeIdentity {
                name: node.name.trim().to_string(),
                endpoint: resolved.to_string(),
                ids,
            }
        })
        .collect();
    entries.sort();
    entries.dedup();
    entries
}

/// Filter to running local `docker-container` builders and collapse aliases
/// pointing at the exact same backing BuildKit nodes (matched by resolved
/// local socket and worker IDs, independent of node name), preferring
/// `Current == true` then lexicographically smallest builder name.
fn deduplicate_local_buildx_builders(
    builders: &[BuildxBuilder],
    local_contexts: &BTreeMap<String, String>,
) -> Vec<BuildxBuilder> {
    let mut winners: BTreeMap<Vec<(String, Vec<String>)>, BuildxBuilder> = BTreeMap::new();
    for candidate in builders {
        if candidate.driver.trim() != "docker-container" || !is_valid_builder_name(&candidate.name)
        {
            continue;
        }
        let Some(nodes) = candidate.nodes.as_deref() else {
            continue;
        };
        if nodes.is_empty()
            || !nodes
                .iter()
                .all(|node| is_local_buildx_node(node, local_contexts))
        {
            continue;
        }
        let mut key: Vec<(String, Vec<String>)> = builder_node_fingerprint(nodes, local_contexts)
            .into_iter()
            .map(|identity| (identity.endpoint, identity.ids))
            .collect();
        key.sort();
        key.dedup();
        match winners.get_mut(&key) {
            Some(existing) => {
                if (!candidate.current, candidate.name.as_str())
                    < (!existing.current, existing.name.as_str())
                {
                    *existing = candidate.clone();
                }
            }
            None => {
                winners.insert(key, candidate.clone());
            }
        }
    }
    let mut selected: Vec<BuildxBuilder> = winners.into_values().collect();
    selected.sort_by(|left, right| left.name.cmp(&right.name));
    selected
}

fn local_buildx_probe_error(
    builders: &[BuildxBuilder],
    local_contexts: &BTreeMap<String, String>,
) -> Option<String> {
    for builder in builders {
        if builder.driver.trim() != "docker-container" || !is_valid_builder_name(&builder.name) {
            continue;
        }
        let Some(nodes) = builder.nodes.as_deref() else {
            continue;
        };
        for node in nodes {
            let endpoint = node.endpoint.trim();
            if !(is_local_docker_host(endpoint) || local_contexts.contains_key(endpoint)) {
                continue;
            }
            let status = node.status.trim();
            if !matches!(status, "running" | "stopped" | "inactive" | "") {
                return Some(format!(
                    "local Docker buildx builder `{}` node `{}` reported status `{status}`",
                    builder.name,
                    node.name.trim()
                ));
            }
        }
    }
    None
}

fn parse_buildx_du(output: &str, builder: &str) -> Result<BuildxUsage> {
    let mut total: Option<(u64, String)> = None;
    let mut reclaimable: Option<u64> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Reclaimable:") {
            let value = value.trim();
            reclaimable = Some(parse_size(value).with_context(|| {
                format!("invalid Docker buildx reclaimable size for `{builder}`: {value}")
            })?);
        } else if let Some(value) = trimmed.strip_prefix("Total:") {
            let value = value.trim();
            let parsed = parse_size(value).with_context(|| {
                format!("invalid Docker buildx total size for `{builder}`: {value}")
            })?;
            total = Some((parsed, value.to_string()));
        }
    }
    let (Some((total_bytes, total_text)), Some(reclaimable_bytes)) = (total, reclaimable) else {
        anyhow::bail!(
            "docker buildx du for builder `{builder}` returned no Total/Reclaimable summary"
        );
    };
    Ok(BuildxUsage {
        total_bytes,
        total_text,
        reclaimable_bytes,
    })
}

fn scan_buildx_findings(home: &Path, docker_config: &Path, host: &str) -> Result<Vec<Finding>> {
    let Some(output) = buildx_ls(home, docker_config, host)? else {
        return Ok(Vec::new());
    };
    let parsed = parse_buildx_ls(&output)?;
    let local_contexts = if needs_context_lookup(&parsed) {
        local_docker_contexts(home, docker_config)?
    } else {
        BTreeMap::new()
    };
    if let Some(error) = local_buildx_probe_error(&parsed, &local_contexts) {
        anyhow::bail!("{error}");
    }
    let builders = deduplicate_local_buildx_builders(&parsed, &local_contexts);
    let mut findings = Vec::new();
    for builder in builders {
        let du_output = docker(
            home,
            docker_config,
            host,
            &["buildx", "du", "--builder", &builder.name],
        )?;
        let usage = parse_buildx_du(&du_output, &builder.name)?;
        if usage.total_bytes == 0 && usage.reclaimable_bytes == 0 {
            continue;
        }
        let (bytes, detail) = if usage.reclaimable_bytes == usage.total_bytes {
            (
                usage.total_bytes,
                format!(
                    "{} total in local buildx builder `{}` on endpoint {host}; \
                     `buildx prune -a` removes every unused cache record; volumes are never touched",
                    usage.total_text, builder.name
                ),
            )
        } else if usage.reclaimable_bytes > 0 {
            (
                usage.reclaimable_bytes,
                format!(
                    "at least this much ({} total) in local buildx builder `{}` on endpoint {host}; \
                     in-use records stay while `buildx prune -a` removes every unused cache record; \
                     volumes are never touched",
                    usage.total_text, builder.name
                ),
            )
        } else {
            (
                usage.total_bytes,
                format!(
                    "up to {} in local buildx builder `{}` on endpoint {host}; \
                     `buildx prune -a` removes unused cache records while keeping in-use records; \
                     volumes are never touched",
                    usage.total_text, builder.name
                ),
            )
        };
        let nodes =
            builder_node_fingerprint(builder.nodes.as_deref().unwrap_or(&[]), &local_contexts);
        findings.push(Finding::command(
            format!("Docker buildx cache ({})", builder.name),
            bytes,
            detail,
            escalate(6, bytes),
            CommandAuthority::DockerBuildxPrune {
                host: host.to_string(),
                builder: builder.name,
                nodes,
            },
        ));
    }
    Ok(findings)
}

fn verify_local_buildx_builder(
    home: &Path,
    docker_config: &Path,
    host: &str,
    builder: &str,
    expected_nodes: &[BuildxNodeIdentity],
) -> Result<()> {
    if expected_nodes.is_empty() {
        anyhow::bail!(
            "refusing buildx prune: builder `{builder}` carries no previewed node identity"
        );
    }
    let Some(output) = buildx_ls(home, docker_config, host)? else {
        anyhow::bail!("refusing buildx prune: `docker buildx` is unavailable");
    };
    let parsed = parse_buildx_ls(&output)?;
    let Some(candidate) = parsed
        .into_iter()
        .find(|candidate| candidate.name == builder)
    else {
        anyhow::bail!("refusing buildx prune: builder `{builder}` no longer exists");
    };
    let single = [candidate];
    let local_contexts = if needs_context_lookup(&single) {
        local_docker_contexts(home, docker_config)?
    } else {
        BTreeMap::new()
    };
    let verified = deduplicate_local_buildx_builders(&single, &local_contexts);
    let [verified_builder] = verified.as_slice() else {
        anyhow::bail!(
            "refusing buildx prune: builder `{builder}` is not a running local docker-container builder"
        );
    };
    let current_nodes = builder_node_fingerprint(
        verified_builder.nodes.as_deref().unwrap_or(&[]),
        &local_contexts,
    );
    if current_nodes != expected_nodes {
        anyhow::bail!(
            "refusing buildx prune: builder `{builder}` changed backing nodes since preview"
        );
    }
    Ok(())
}

const MAX_DOCKER_CONFIG_BYTES: u64 = 1024 * 1024;

fn is_safe_absolute_dir(path: &Path) -> bool {
    let encoded = path.as_os_str().as_encoded_bytes();
    path.is_absolute()
        && !encoded.contains(&b'\n')
        && path
            .components()
            .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
        && !encoded
            .split(|byte| *byte == b'/')
            .any(|segment| segment == b"." || segment == b"..")
}

fn validate_trusted_ancestors(candidate: &Path, candidate_exists: bool, label: &str) -> Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let current_uid = rustix::process::geteuid().as_raw();
    let mut deepest_existing = None;
    for ancestor in candidate.ancestors().skip(1) {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if deepest_existing.is_none() {
                    deepest_existing = Some(ancestor);
                }
                if metadata.uid() != current_uid && metadata.uid() != 0 {
                    anyhow::bail!(
                        "refusing {label} with ancestor {} not owned by current user or root",
                        ancestor.display()
                    );
                }
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if !metadata.file_type().is_dir() {
                    anyhow::bail!(
                        "refusing {label} with non-directory ancestor {}",
                        ancestor.display()
                    );
                }
                let mode = metadata.permissions().mode();
                let others_write = mode & 0o022 != 0;
                if others_write && (mode & 0o1000 == 0) {
                    anyhow::bail!(
                        "refusing {label} with group- or world-writable ancestor {}",
                        ancestor.display()
                    );
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("cannot inspect ancestor {} of {label}", ancestor.display())
                });
            }
        }
    }
    let Some(existing_parent) = deepest_existing else {
        anyhow::bail!("refusing {label} with no existing ancestor directory");
    };
    let canonical_parent = existing_parent.canonicalize().with_context(|| {
        format!(
            "cannot resolve ancestor {} of {label}",
            existing_parent.display()
        )
    })?;
    for (depth, ancestor) in canonical_parent.ancestors().enumerate() {
        let metadata = std::fs::symlink_metadata(ancestor).with_context(|| {
            format!(
                "cannot inspect canonical ancestor {} of {label}",
                ancestor.display()
            )
        })?;
        if !metadata.file_type().is_dir() {
            anyhow::bail!(
                "refusing {label} with non-directory canonical ancestor {}",
                ancestor.display()
            );
        }
        if metadata.uid() != current_uid && metadata.uid() != 0 {
            anyhow::bail!(
                "refusing {label} with canonical ancestor {} not owned by current user or root",
                ancestor.display()
            );
        }
        let mode = metadata.permissions().mode();
        let others_write = mode & 0o022 != 0;
        let sticky_allowed = candidate_exists || depth > 0;
        if others_write && (!sticky_allowed || mode & 0o1000 == 0) {
            anyhow::bail!(
                "refusing {label} with writable ancestor {}",
                ancestor.display()
            );
        }
    }
    if canonical_parent
        .as_os_str()
        .as_encoded_bytes()
        .contains(&b'\n')
    {
        anyhow::bail!(
            "refusing {label} with newline in resolved ancestor {}",
            canonical_parent.display()
        );
    }
    super::project::refuse_path_acl_grants(canonical_parent.ancestors(), &canonical_parent, label)?;
    Ok(())
}

fn validate_docker_config_dir_from(
    raw_docker_config: Option<&std::ffi::OsStr>,
    home: &Path,
) -> Result<PathBuf> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let candidate = match raw_docker_config.filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => home.join(".docker"),
    };
    if !is_safe_absolute_dir(&candidate) {
        anyhow::bail!(
            "refusing relative or non-normalized DOCKER_CONFIG: {}",
            candidate.display()
        );
    }
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                anyhow::bail!(
                    "refusing symlinked or non-directory DOCKER_CONFIG: {}",
                    candidate.display()
                );
            }
            validate_trusted_ancestors(&candidate, true, "DOCKER_CONFIG")?;
            let current_uid = rustix::process::geteuid().as_raw();
            if metadata.uid() != current_uid && metadata.uid() != 0 {
                anyhow::bail!(
                    "refusing DOCKER_CONFIG owned by UID {} (expected {current_uid} or root): {}",
                    metadata.uid(),
                    candidate.display()
                );
            }
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "refusing group- or world-writable DOCKER_CONFIG: {}",
                    candidate.display()
                );
            }
            super::project::refuse_path_acl_grants(
                [candidate.as_path()],
                &candidate,
                "DOCKER_CONFIG",
            )?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            validate_trusted_ancestors(&candidate, false, "DOCKER_CONFIG")?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect DOCKER_CONFIG {}", candidate.display()));
        }
    }
    Ok(candidate)
}

fn read_docker_config_json(
    source_config: &Path,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>> {
    use rustix::fs::{FileType, Mode, OFlags, fstat, open};
    use std::io::Read as _;

    let config_path = source_config.join("config.json");
    let descriptor = match open(
        &config_path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(errno) => {
            return Err(std::io::Error::from(errno))
                .with_context(|| format!("cannot open Docker config {}", config_path.display()));
        }
    };
    let stat = fstat(&descriptor)
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot inspect Docker config {}", config_path.display()))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        anyhow::bail!(
            "refusing non-regular Docker config {}",
            config_path.display()
        );
    }
    let current_uid = rustix::process::geteuid().as_raw();
    if stat.st_uid != current_uid && stat.st_uid != 0 {
        anyhow::bail!(
            "refusing Docker config owned by UID {} (expected {current_uid} or root): {}",
            stat.st_uid,
            config_path.display()
        );
    }
    if u32::from(stat.st_mode) & 0o022 != 0 {
        anyhow::bail!(
            "refusing group- or world-writable Docker config {}",
            config_path.display()
        );
    }
    super::project::refuse_path_acl_grants([config_path.as_path()], &config_path, "Docker config")?;
    let mut file = std::fs::File::from(descriptor);
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_DOCKER_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("cannot read Docker config {}", config_path.display()))?;
    if bytes.len() as u64 > MAX_DOCKER_CONFIG_BYTES {
        anyhow::bail!(
            "Docker config exceeds {MAX_DOCKER_CONFIG_BYTES} bytes: {}",
            config_path.display()
        );
    }
    let contents = String::from_utf8(bytes).with_context(|| {
        format!(
            "Docker config is not valid UTF-8: {}",
            config_path.display()
        )
    })?;
    let parsed: serde_json::Value = serde_json::from_str(&contents)
        .with_context(|| format!("invalid Docker config JSON: {}", config_path.display()))?;
    let object = parsed.as_object().cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Docker config is not a JSON object: {}",
            config_path.display()
        )
    })?;
    Ok(Some(object))
}

fn validate_docker_plugin_dir(candidate: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    if !is_safe_absolute_dir(candidate) {
        anyhow::bail!(
            "refusing unsafe Docker CLI plugin directory (must be an absolute path without `.` or `..`): {}",
            candidate.display()
        );
    }
    let link_metadata = match std::fs::symlink_metadata(candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            validate_trusted_ancestors(candidate, false, "Docker CLI plugin directory")?;
            return Ok(());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot inspect Docker CLI plugin directory {}",
                    candidate.display()
                )
            });
        }
    };
    validate_trusted_ancestors(candidate, true, "Docker CLI plugin directory")?;
    let current_uid = rustix::process::geteuid().as_raw();
    if link_metadata.uid() != current_uid && link_metadata.uid() != 0 {
        anyhow::bail!(
            "refusing Docker CLI plugin directory not owned by current user or root: {}",
            candidate.display()
        );
    }
    let canonical = match candidate.canonicalize() {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            anyhow::bail!(
                "refusing broken symlink in Docker CLI plugin directories: {}",
                candidate.display()
            );
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot resolve Docker CLI plugin directory {}",
                    candidate.display()
                )
            });
        }
    };
    validate_trusted_ancestors(&canonical, true, "Docker CLI plugin directory target")?;
    let metadata = std::fs::symlink_metadata(&canonical).with_context(|| {
        format!(
            "cannot inspect resolved Docker CLI plugin directory {}",
            canonical.display()
        )
    })?;
    if !metadata.is_dir() {
        anyhow::bail!(
            "refusing non-directory Docker CLI plugin path: {}",
            candidate.display()
        );
    }
    if metadata.uid() != current_uid && metadata.uid() != 0 {
        anyhow::bail!(
            "refusing Docker CLI plugin directory target not owned by current user or root: {}",
            candidate.display()
        );
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!(
            "refusing group- or world-writable Docker CLI plugin directory: {}",
            candidate.display()
        );
    }
    if canonical.as_os_str().as_encoded_bytes().contains(&b'\n') {
        anyhow::bail!(
            "refusing Docker CLI plugin directory with newline in resolved path: {}",
            canonical.display()
        );
    }
    super::project::refuse_path_acl_grants(
        [canonical.as_path()],
        &canonical,
        "Docker CLI plugin directory",
    )?;
    for entry in std::fs::read_dir(&canonical).with_context(|| {
        format!(
            "cannot list Docker CLI plugin directory {}",
            canonical.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot read entry in Docker CLI plugin directory {}",
                canonical.display()
            )
        })?;
        let name = entry.file_name();
        if !name.as_encoded_bytes().starts_with(b"docker-") {
            continue;
        }
        let entry_path = entry.path();
        let entry_link_meta = std::fs::symlink_metadata(&entry_path).with_context(|| {
            format!(
                "cannot inspect Docker CLI plugin entry {}",
                entry_path.display()
            )
        })?;
        if entry_link_meta.uid() != current_uid && entry_link_meta.uid() != 0 {
            anyhow::bail!(
                "refusing Docker CLI plugin entry not owned by current user or root: {}",
                entry_path.display()
            );
        }
        let resolved_plugin = match entry_path.canonicalize() {
            Ok(resolved) => resolved,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                anyhow::bail!(
                    "refusing broken symlink for Docker CLI plugin entry: {}",
                    entry_path.display()
                );
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "cannot resolve Docker CLI plugin entry {}",
                        entry_path.display()
                    )
                });
            }
        };
        if resolved_plugin
            .as_os_str()
            .as_encoded_bytes()
            .contains(&b'\n')
        {
            anyhow::bail!(
                "refusing Docker CLI plugin executable with newline in resolved path: {}",
                resolved_plugin.display()
            );
        }
        validate_trusted_ancestors(&resolved_plugin, true, "Docker CLI plugin executable")?;
        let target_meta = std::fs::symlink_metadata(&resolved_plugin).with_context(|| {
            format!(
                "cannot inspect resolved Docker CLI plugin executable {}",
                resolved_plugin.display()
            )
        })?;
        if !target_meta.is_file() {
            anyhow::bail!(
                "refusing non-regular Docker CLI plugin entry: {}",
                entry_path.display()
            );
        }
        if target_meta.uid() != current_uid && target_meta.uid() != 0 {
            anyhow::bail!(
                "refusing Docker CLI plugin executable not owned by current user or root: {}",
                resolved_plugin.display()
            );
        }
        if target_meta.permissions().mode() & 0o022 != 0 {
            anyhow::bail!(
                "refusing group- or world-writable Docker CLI plugin executable: {}",
                resolved_plugin.display()
            );
        }
        super::project::refuse_path_acl_grants(
            [resolved_plugin.as_path()],
            &resolved_plugin,
            "Docker CLI plugin executable",
        )?;
    }
    Ok(())
}

#[derive(Debug)]
struct ValidatedDockerConfig {
    source_config: PathBuf,
    plugin_dirs: Vec<String>,
    current_context: Option<String>,
}

fn validate_docker_config_from(
    raw_docker_config: Option<&std::ffi::OsStr>,
    home: &Path,
) -> Result<ValidatedDockerConfig> {
    let source_config = validate_docker_config_dir_from(raw_docker_config, home)?;
    let mut plugin_dirs = Vec::new();
    let mut current_context = None;
    let mut push_validated = |candidate: PathBuf| -> Result<()> {
        validate_docker_plugin_dir(&candidate)?;
        let text = candidate.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "refusing non-UTF-8 Docker CLI plugin directory: {}",
                candidate.display()
            )
        })?;
        if !plugin_dirs.iter().any(|existing| existing == text) {
            plugin_dirs.push(text.to_owned());
        }
        Ok(())
    };
    // Preserve Docker CLI's exact plugin lookup order
    // (`cli-plugins/manager/manager.go` + `manager_unix.go`): configured
    // safe absolute `cliPluginsExtraDirs` first, then `$DOCKER_CONFIG/cli-plugins`,
    // failing closed on any relative, unowned, or group/world-writable entry so
    // neither scan probes nor isolated prunes load untrusted plugins.
    if let Some(map) = read_docker_config_json(&source_config)? {
        if let Some(value) = map
            .get("currentContext")
            .or_else(|| {
                map.iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("currentContext"))
                    .map(|(_, value)| value)
            })
            .and_then(|value| value.as_str())
            && !value.is_empty()
        {
            current_context = Some(value.to_owned());
        }
        if let Some(extra) = map
            .get("cliPluginsExtraDirs")
            .or_else(|| {
                map.iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("cliPluginsExtraDirs"))
                    .map(|(_, value)| value)
            })
            .and_then(|value| value.as_array())
        {
            for entry in extra {
                let Some(text) = entry.as_str() else {
                    anyhow::bail!(
                        "refusing non-string entry in Docker config cliPluginsExtraDirs: {}",
                        source_config.join("config.json").display()
                    );
                };
                if text.is_empty() {
                    continue;
                }
                push_validated(PathBuf::from(text))?;
            }
        }
    }
    push_validated(source_config.join("cli-plugins"))?;
    Ok(ValidatedDockerConfig {
        source_config,
        plugin_dirs,
        current_context,
    })
}

/// Read-only validation and extraction of safe absolute Docker CLI plugin
/// directories from `DOCKER_CONFIG` and its `config.json`. Called before
/// `journal::begin` so an invalid or hostile config fails closed without
/// acquiring `journal.lock` or writing a journal `attempt` record.
pub(crate) fn resolve_docker_cli_plugin_dirs(home: &Path) -> Result<Vec<String>> {
    let raw_docker_config = std::env::var_os("DOCKER_CONFIG");
    resolve_docker_cli_plugin_dirs_from(raw_docker_config.as_deref(), home)
}

fn resolve_docker_cli_plugin_dirs_from(
    raw_docker_config: Option<&std::ffi::OsStr>,
    home: &Path,
) -> Result<Vec<String>> {
    Ok(validate_docker_config_from(raw_docker_config, home)?.plugin_dirs)
}

fn validated_docker_state_subdir(source_config: &Path, name: &str) -> Result<Option<PathBuf>> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let path = source_config.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                anyhow::bail!(
                    "refusing symlinked or non-directory Docker state path: {}",
                    path.display()
                );
            }
            let current_uid = rustix::process::geteuid().as_raw();
            if metadata.uid() != current_uid && metadata.uid() != 0 {
                anyhow::bail!(
                    "refusing Docker state path owned by UID {} (expected {current_uid} or root): {}",
                    metadata.uid(),
                    path.display()
                );
            }
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "refusing group- or world-writable Docker state path: {}",
                    path.display()
                );
            }
            let canonical = path
                .canonicalize()
                .with_context(|| format!("cannot resolve Docker state path {}", path.display()))?;
            if canonical.as_os_str().as_encoded_bytes().contains(&b'\n') {
                anyhow::bail!(
                    "refusing Docker state path with newline in resolved path: {}",
                    canonical.display()
                );
            }
            super::project::refuse_path_acl_grants(
                [canonical.as_path()],
                &canonical,
                "Docker state path",
            )?;
            Ok(Some(canonical))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("cannot inspect Docker state path {}", path.display())),
    }
}

/// Materializes a deterministic private `0700` `DOCKER_CONFIG` snapshot under
/// `std::env::temp_dir()` containing ONLY the validated `config.json`
/// (`cliPluginsExtraDirs` and optional `currentContext`, no `aliases` or
/// ambient fields) plus verified symlinks to existing `contexts/` and `buildx/`
/// directories. Every `docker` probe runs against this snapshot so Docker never
/// rereads ambient `$DOCKER_CONFIG/config.json` after validation.
fn isolated_docker_scan_dir(home: &Path) -> Result<PathBuf> {
    let raw_docker_config = std::env::var_os("DOCKER_CONFIG");
    isolated_docker_scan_dir_from(raw_docker_config.as_deref(), home, &std::env::temp_dir())
}

fn isolated_docker_scan_dir_from(
    raw_docker_config: Option<&std::ffi::OsStr>,
    home: &Path,
    base: &Path,
) -> Result<PathBuf> {
    use rustix::fs::{Mode, OFlags, fchmod, mkdirat, open, openat};
    use sha2::{Digest as _, Sha256};
    use std::io::{Read as _, Write as _};
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let validated = validate_docker_config_from(raw_docker_config, home)?;
    let contexts_target = validated_docker_state_subdir(&validated.source_config, "contexts")?;
    let buildx_target = validated_docker_state_subdir(&validated.source_config, "buildx")?;

    let mut config_map = serde_json::Map::new();
    config_map.insert(
        "cliPluginsExtraDirs".to_string(),
        serde_json::Value::Array(
            validated
                .plugin_dirs
                .iter()
                .map(|entry| serde_json::Value::String(entry.clone()))
                .collect(),
        ),
    );
    if let Some(current_context) = validated.current_context {
        config_map.insert(
            "currentContext".to_string(),
            serde_json::Value::String(current_context),
        );
    }
    let config_bytes = serde_json::to_vec(&serde_json::Value::Object(config_map))
        .context("cannot serialize validated Docker scan config")?;

    let canonical_base = base
        .canonicalize()
        .with_context(|| format!("cannot resolve temp directory {}", base.display()))?;
    let current_uid = rustix::process::geteuid().as_raw();
    for ancestor in canonical_base.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)
            .with_context(|| format!("cannot inspect temp ancestor {}", ancestor.display()))?;
        let others_write = metadata.permissions().mode() & 0o022 != 0;
        let trusted = metadata.file_type().is_dir()
            && (metadata.uid() == current_uid || metadata.uid() == 0)
            && (!others_write || metadata.permissions().mode() & 0o1000 != 0);
        if !trusted {
            anyhow::bail!(
                "refusing Docker scan temp parent {}: another account could change it",
                ancestor.display()
            );
        }
    }

    if canonical_base
        .as_os_str()
        .as_encoded_bytes()
        .contains(&b'\n')
    {
        anyhow::bail!(
            "refusing Docker scan temp parent {}: its name holds a newline",
            canonical_base.display()
        );
    }
    super::project::refuse_path_acl_grants(
        canonical_base.ancestors(),
        &canonical_base,
        "Docker scan temp parent",
    )?;

    let mut hasher = Sha256::new();
    hasher.update(b"devtrim-docker-scan-v1\0");
    hasher.update(&config_bytes);
    hasher.update(b"\0");
    if let Some(target) = &contexts_target {
        hasher.update(target.as_os_str().as_encoded_bytes());
    }
    hasher.update(b"\0");
    if let Some(target) = &buildx_target {
        hasher.update(target.as_os_str().as_encoded_bytes());
    }
    let digest = hasher.finalize();
    let mut suffix = String::with_capacity(16);
    for byte in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(&mut suffix, "{byte:02x}");
    }
    let dir_name = format!("devtrim-docker-scan-{current_uid}-{suffix}");
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let base_descriptor = open(&canonical_base, dir_flags, Mode::empty())
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot open temp directory {}", canonical_base.display()))?;
    let base_file = std::fs::File::from(base_descriptor);
    match mkdirat(&base_file, &dir_name, Mode::from(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(errno) => {
            return Err(std::io::Error::from(errno))
                .with_context(|| format!("cannot create Docker scan directory `{dir_name}`"));
        }
    }
    let scan_descriptor = openat(&base_file, &dir_name, dir_flags, Mode::empty())
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot open Docker scan directory `{dir_name}`"))?;
    let scan_file = std::fs::File::from(scan_descriptor);
    let scan_metadata = scan_file
        .metadata()
        .with_context(|| format!("cannot inspect Docker scan directory `{dir_name}`"))?;
    if !scan_metadata.is_dir()
        || scan_metadata.uid() != current_uid
        || scan_metadata.permissions().mode() & 0o077 != 0
    {
        anyhow::bail!("refusing untrusted Docker scan directory `{dir_name}`");
    }
    fchmod(&scan_file, Mode::from(0o700))
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot tighten permissions on `{dir_name}`"))?;

    let scan_dir = canonical_base.join(&dir_name);
    let config_flags =
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    match openat(&scan_file, "config.json", config_flags, Mode::from(0o600)) {
        Ok(descriptor) => {
            let mut file = std::fs::File::from(descriptor);
            let metadata = file
                .metadata()
                .context("cannot inspect isolated Docker scan config.json")?;
            if !metadata.is_file() || metadata.uid() != current_uid {
                anyhow::bail!("isolated Docker scan config.json is not owned by the current user");
            }
            fchmod(&file, Mode::from(0o600))
                .map_err(std::io::Error::from)
                .context("cannot tighten isolated Docker scan config.json permissions")?;
            file.write_all(&config_bytes)
                .context("cannot write isolated Docker scan config.json")?;
            file.sync_all()
                .context("cannot sync isolated Docker scan config.json")?;
        }
        Err(rustix::io::Errno::EXIST) => {
            let existing = openat(
                &scan_file,
                "config.json",
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)
            .context("cannot open existing isolated Docker scan config.json")?;
            let file = std::fs::File::from(existing);
            let metadata = file
                .metadata()
                .context("cannot inspect existing isolated Docker scan config.json")?;
            if !metadata.is_file()
                || metadata.uid() != current_uid
                || metadata.permissions().mode() & 0o077 != 0
            {
                anyhow::bail!("refusing untrusted isolated Docker scan config.json");
            }
            let mut actual = Vec::new();
            std::io::Read::take(file, MAX_DOCKER_CONFIG_BYTES + 1)
                .read_to_end(&mut actual)
                .context("cannot read existing isolated Docker scan config.json")?;
            if actual != config_bytes {
                anyhow::bail!("refusing modified isolated Docker scan config.json");
            }
        }
        Err(errno) => {
            return Err(std::io::Error::from(errno))
                .context("cannot create isolated Docker scan config.json");
        }
    }

    let mut expected_entries = vec!["config.json"];
    for (link_name, maybe_target) in [
        ("contexts", contexts_target.as_deref()),
        ("buildx", buildx_target.as_deref()),
    ] {
        let Some(target) = maybe_target else {
            continue;
        };
        expected_entries.push(link_name);
        let link_path = scan_dir.join(link_name);
        match std::os::unix::fs::symlink(target, &link_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let link_metadata = std::fs::symlink_metadata(&link_path).with_context(|| {
                    format!("cannot inspect isolated Docker scan link `{link_name}`")
                })?;
                if !link_metadata.file_type().is_symlink() || link_metadata.uid() != current_uid {
                    anyhow::bail!("refusing untrusted isolated Docker scan link `{link_name}`");
                }
                let existing_target = std::fs::read_link(&link_path).with_context(|| {
                    format!("cannot read isolated Docker scan link `{link_name}`")
                })?;
                if existing_target != target {
                    anyhow::bail!("refusing retargeted isolated Docker scan link `{link_name}`");
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("cannot link isolated Docker scan `{link_name}` directory")
                });
            }
        }
    }

    for entry in std::fs::read_dir(&scan_dir).with_context(|| {
        format!(
            "cannot list isolated Docker scan directory {}",
            scan_dir.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot read isolated Docker scan directory {}",
                scan_dir.display()
            )
        })?;
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            anyhow::bail!("refusing unexpected entry in isolated Docker scan directory");
        };
        if !expected_entries.contains(&name_text) {
            anyhow::bail!(
                "refusing unexpected entry `{name_text}` in isolated Docker scan directory"
            );
        }
    }
    let scan_config_path = scan_dir.join("config.json");
    super::project::refuse_path_acl_grants(
        [scan_dir.as_path(), scan_config_path.as_path()],
        &scan_dir,
        "isolated Docker scan directory",
    )?;
    Ok(scan_dir)
}

/// RAII guard for a per-attempt private `DOCKER_CONFIG` directory under
/// `buildx-pin/attempt-<pid>-<seq>`. Keeps the tree alive across `docker`
/// execution and removes it on drop through `src/ops/mod.rs`'s verified sink.
pub(crate) struct PrivateDockerConfigGuard {
    path: PathBuf,
    home: PathBuf,
    identity: crate::safety::FileIdentity,
}

impl PrivateDockerConfigGuard {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateDockerConfigGuard {
    fn drop(&mut self) {
        super::cleanup_private_docker_config(&self.path, &self.home, self.identity);
    }
}

struct PrivateDockerConfigAttempt {
    guard: PrivateDockerConfigGuard,
    buildx_dir: std::fs::File,
    current_uid: u32,
}

fn open_private_dir_child(
    parent: &std::fs::File,
    name: &str,
    current_uid: u32,
) -> Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, fchmod, mkdirat, openat};
    use std::os::unix::fs::MetadataExt as _;

    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    match mkdirat(parent, name, Mode::from(0o700)) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => {}
        Err(errno) => {
            return Err(std::io::Error::from(errno))
                .with_context(|| format!("cannot create private buildx directory `{name}`"));
        }
    }
    let descriptor = openat(parent, name, dir_flags, Mode::empty())
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot open private buildx directory `{name}`"))?;
    let directory = std::fs::File::from(descriptor);
    let metadata = directory
        .metadata()
        .with_context(|| format!("cannot inspect private buildx directory `{name}`"))?;
    if !metadata.is_dir() || metadata.uid() != current_uid {
        anyhow::bail!("private buildx directory `{name}` is not owned by the current user");
    }
    fchmod(&directory, Mode::from(0o700))
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot tighten permissions on `{name}`"))?;
    Ok(directory)
}

fn write_private_json_file(
    parent: &std::fs::File,
    name: &str,
    payload: &serde_json::Value,
    current_uid: u32,
) -> Result<()> {
    use rustix::fs::{Mode, OFlags, fchmod, openat};
    use std::io::Write as _;
    use std::os::unix::fs::MetadataExt as _;

    let file_flags =
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let descriptor = openat(parent, name, file_flags, Mode::from(0o600))
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot write private buildx file `{name}`"))?;
    let mut file = std::fs::File::from(descriptor);
    let metadata = file
        .metadata()
        .with_context(|| format!("cannot inspect private buildx file `{name}`"))?;
    if !metadata.is_file() || metadata.uid() != current_uid {
        anyhow::bail!("private buildx file `{name}` is not owned by the current user");
    }
    fchmod(&file, Mode::from(0o600))
        .map_err(std::io::Error::from)
        .with_context(|| format!("cannot tighten permissions on `{name}`"))?;
    let encoded =
        serde_json::to_vec(payload).with_context(|| format!("cannot serialize `{name}`"))?;
    file.write_all(&encoded)
        .with_context(|| format!("cannot write `{name}`"))?;
    file.sync_all()
        .with_context(|| format!("cannot sync `{name}`"))?;
    Ok(())
}

fn allocate_private_docker_config(
    ctx: &Ctx,
    plugin_dirs: &[String],
) -> Result<PrivateDockerConfigAttempt> {
    use rustix::fs::{Mode, OFlags, fchmod, mkdirat, open, openat};
    use std::os::unix::fs::MetadataExt as _;

    let state_dir = ctx
        .journal_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("journal path has no parent directory"))?;
    let current_uid = rustix::process::getuid().as_raw();
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let state_file = std::fs::File::from(
        open(state_dir, dir_flags, Mode::empty())
            .map_err(std::io::Error::from)
            .with_context(|| format!("cannot open state directory {}", state_dir.display()))?,
    );
    let canonical_state_dir = state_dir
        .canonicalize()
        .with_context(|| format!("cannot resolve state directory {}", state_dir.display()))?;
    if canonical_state_dir
        .as_os_str()
        .as_encoded_bytes()
        .contains(&b'\n')
    {
        anyhow::bail!(
            "refusing Docker state directory {}: its path holds a newline",
            canonical_state_dir.display()
        );
    }
    validate_trusted_ancestors(&canonical_state_dir, true, "Docker private state directory")?;
    super::project::refuse_path_acl_grants(
        canonical_state_dir.ancestors(),
        &canonical_state_dir,
        "Docker private state directory",
    )?;
    let pin_base = open_private_dir_child(&state_file, "buildx-pin", current_uid)?;
    // Allocate a fresh per-attempt subdirectory (`O_EXCL` via `mkdirat`) so
    // concurrent applies never share or overwrite each other's isolated config,
    // and write files with `O_CREAT | O_EXCL` (never `O_TRUNC`).
    static PIN_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let pid = std::process::id();
    let (attempt_name, pin_root, attempt_identity) = {
        let mut chosen = None;
        for _ in 0..64 {
            let sequence = PIN_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let candidate = format!("attempt-{pid}-{sequence}");
            match mkdirat(&pin_base, &candidate, Mode::from(0o700)) {
                Ok(()) => {
                    let descriptor = openat(&pin_base, &candidate, dir_flags, Mode::empty())
                        .map_err(std::io::Error::from)
                        .with_context(|| {
                            format!("cannot open private buildx directory `{candidate}`")
                        })?;
                    let directory = std::fs::File::from(descriptor);
                    let metadata = directory.metadata().with_context(|| {
                        format!("cannot inspect private buildx directory `{candidate}`")
                    })?;
                    if !metadata.is_dir() || metadata.uid() != current_uid {
                        anyhow::bail!(
                            "private buildx directory `{candidate}` is not owned by the current user"
                        );
                    }
                    fchmod(&directory, Mode::from(0o700))
                        .map_err(std::io::Error::from)
                        .with_context(|| format!("cannot tighten permissions on `{candidate}`"))?;
                    let identity = crate::safety::FileIdentity::from_std_metadata(&metadata);
                    chosen = Some((candidate, directory, identity));
                    break;
                }
                Err(rustix::io::Errno::EXIST) => continue,
                Err(errno) => {
                    return Err(std::io::Error::from(errno)).with_context(|| {
                        format!("cannot create private buildx directory `{candidate}`")
                    });
                }
            }
        }
        chosen.ok_or_else(|| {
            anyhow::anyhow!("cannot allocate unique private buildx directory under state dir")
        })?
    };
    let guard = PrivateDockerConfigGuard {
        path: canonical_state_dir.join("buildx-pin").join(attempt_name),
        home: ctx.home.clone(),
        identity: attempt_identity,
    };
    let buildx_dir = open_private_dir_child(&pin_root, "buildx", current_uid)?;
    let config_payload = serde_json::json!({
        "cliPluginsExtraDirs": plugin_dirs,
    });
    write_private_json_file(&pin_root, "config.json", &config_payload, current_uid)?;
    let pin_base_path = canonical_state_dir.join("buildx-pin");
    let buildx_path = guard.path().join("buildx");
    let config_path = guard.path().join("config.json");
    super::project::refuse_path_acl_grants(
        [
            pin_base_path.as_path(),
            guard.path(),
            buildx_path.as_path(),
            config_path.as_path(),
        ],
        guard.path(),
        "private Docker config directory",
    )?;
    Ok(PrivateDockerConfigAttempt {
        guard,
        buildx_dir,
        current_uid,
    })
}

/// Writes an isolated `0700` `DOCKER_CONFIG` tree inside devtrim's private
/// state directory containing ONLY `config.json` (safe absolute CLI plugin
/// discovery paths, no credentials, no `aliases`) and an empty `0700` `buildx/`
/// directory, with no `contexts/` directory. Running `docker builder prune`
/// with this `DOCKER_CONFIG` and `BUILDX_CONFIG=<isolated>/buildx` prevents
/// Buildx aliases or current-builder state from overriding `--host`.
pub(crate) fn write_isolated_builder_config(
    ctx: &Ctx,
    plugin_dirs: &[String],
) -> Result<PrivateDockerConfigGuard> {
    let attempt = allocate_private_docker_config(ctx, plugin_dirs)?;
    Ok(attempt.guard)
}

/// Writes an isolated `0700` `DOCKER_CONFIG` tree inside devtrim's private
/// state directory containing ONLY `config.json` (CLI plugin discovery paths,
/// no credentials) and `buildx/instances/<builder>` pinned to the verified
/// `docker-container` driver and resolved local `unix://` node endpoints, with
/// no `contexts/` directory. Running `docker buildx prune` with this
/// `DOCKER_CONFIG` prevents a concurrent builder or context rebind from
/// redirecting the prune to an unpreviewed or remote worker.
pub(crate) fn write_pinned_buildx_config(
    ctx: &Ctx,
    builder: &str,
    nodes: &[BuildxNodeIdentity],
    plugin_dirs: &[String],
) -> Result<PrivateDockerConfigGuard> {
    if !is_valid_builder_name(builder) {
        anyhow::bail!("refusing invalid Docker buildx builder name `{builder}`");
    }
    if nodes.is_empty() {
        anyhow::bail!("refusing Docker buildx builder `{builder}` with no verified nodes");
    }
    for node in nodes {
        if node.name.is_empty()
            || !is_valid_builder_name(&node.name)
            || !is_local_docker_host(&node.endpoint)
        {
            anyhow::bail!(
                "refusing unverified Docker buildx node `{}` at `{}`",
                node.name,
                node.endpoint
            );
        }
    }

    let attempt = allocate_private_docker_config(ctx, plugin_dirs)?;
    let instances_dir =
        open_private_dir_child(&attempt.buildx_dir, "instances", attempt.current_uid)?;
    let instance_nodes: Vec<serde_json::Value> = nodes
        .iter()
        .map(|node| {
            serde_json::json!({
                "Name": node.name,
                "Endpoint": node.endpoint,
            })
        })
        .collect();
    let instance_payload = serde_json::json!({
        "Name": builder,
        "Driver": "docker-container",
        "Nodes": instance_nodes,
        "Dynamic": false,
    });
    write_private_json_file(
        &instances_dir,
        builder,
        &instance_payload,
        attempt.current_uid,
    )?;
    Ok(attempt.guard)
}

/// Blocks actually allocated on the host, not the logical length.
///
/// A VM disk image is sparse, so the two differ by orders of magnitude: a real
/// OrbStack image measured 926 GB logical against 35 GB allocated, which is
/// why every devtrim size is allocated bytes. Returns `None` when the path is
/// absent or is not a regular file; a stat failure for a path that does exist
/// fails closed.
fn allocated_bytes(path: &Path) -> Result<Option<u64>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    };
    if !metadata.file_type().is_file() {
        return Ok(None);
    }
    crate::safety::allocated_bytes(&metadata, path).map(Some)
}

/// Report-only disclosure of the host cost of each present runtime disk image.
///
/// These findings are never actionable: compacting or deleting a live VM image
/// is the runtime's job, and doing it from here would destroy every container
/// and volume on the machine.
fn vm_disk_findings(home: &Path) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    for (runtime, relative) in VM_DISK_IMAGES {
        let path = home.join(relative);
        let Some(allocated) = allocated_bytes(&path)? else {
            continue;
        };
        if allocated == 0 {
            continue;
        }
        let logical = std::fs::symlink_metadata(&path)
            .with_context(|| format!("cannot inspect {}", path.display()))?
            .len();
        findings.push(Finding::new(
            format!("{runtime} VM disk image"),
            Some(path),
            allocated,
            format!(
                "EXCLUDED: host-side virtual disk for the local Docker runtime, listed for visibility only. \
                 Shown size is allocated blocks; the file is sparse and reports {logical} logical bytes. \
                 Pruning images or build cache frees space inside the VM; this file shrinks only once \
                 {runtime} returns those blocks to macOS, so scan again to measure what the host got back"
            ),
            0,
            Action::None,
        ));
    }
    Ok(findings)
}

fn parse_system_df(output: &str, host: &str) -> Result<Vec<Finding>> {
    if output.trim().is_empty() {
        anyhow::bail!("docker system df returned empty output");
    }
    let mut findings = Vec::new();
    for (index, line) in output.lines().enumerate() {
        let mut columns = line.split('\t');
        let (Some(kind), Some(total), Some(reclaimable), Some(active), None) = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        ) else {
            anyhow::bail!("invalid Docker system df row {}", index.saturating_add(1));
        };
        if kind.is_empty() || total.is_empty() || reclaimable.is_empty() {
            anyhow::bail!("invalid Docker system df row {}", index.saturating_add(1));
        }
        let (label, bytes, effect, authority) = match kind {
            "Images" => (
                "Docker Images reclaimable",
                parse_size(reclaimable)
                    .with_context(|| format!("invalid Docker reclaimable size: {reclaimable}"))?,
                "removes every image not referenced by a container, including untagged local builds"
                    .to_string(),
                CommandAuthority::DockerImagePrune {
                    host: host.to_string(),
                },
            ),
            "Build Cache" => {
                let (bytes, effect) = build_cache_estimate(total, reclaimable, active)?;
                (
                    "Docker Build Cache reclaimable",
                    bytes,
                    effect,
                    CommandAuthority::DockerBuilderPrune {
                        host: host.to_string(),
                    },
                )
            }
            _ => continue,
        };
        if bytes == 0 {
            continue;
        }
        findings.push(Finding::command(
            label,
            bytes,
            format!("{total} total on local endpoint {host}; {effect}; volumes are never touched"),
            escalate(6, bytes),
            authority,
        ));
    }
    Ok(findings)
}

fn docker_on_path() -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|directory| {
        let candidate = if directory.as_os_str().is_empty() {
            PathBuf::from("docker")
        } else {
            directory.join("docker")
        };
        std::fs::metadata(candidate)
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

impl Op for Docker {
    fn name(&self) -> &'static str {
        "docker"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        let _ = resolve_docker_cli_plugin_dirs(&ctx.home)?;
        // The host image is disclosed even when the daemon is unreachable. A
        // stopped VM is precisely when its disk image is invisible to `docker`
        // and still occupying the host, so making this disclosure depend on a
        // live daemon would hide the cost in the one state that matters most.
        let mut findings = vm_disk_findings(&ctx.home)?;
        if !docker_on_path() {
            return Ok(findings);
        }
        let scan_docker_config = isolated_docker_scan_dir(&ctx.home)?;
        // Only an absent `docker` binary becomes an empty success, via
        // `docker_host`'s `ErrorKind::NotFound` handling. Every other failure —
        // a refused endpoint, an unreachable daemon, a malformed response —
        // propagates, because a silently shorter plan is indistinguishable from
        // a machine with nothing to reclaim (`CODING_STANDARDS.md` S8).
        //
        // The cost of that strictness is real and worth stating: when `docker`
        // is installed but its daemon is down, this op errors and the host
        // image findings above are lost with it. The error names the daemon,
        // which is itself the actionable next step, and carries each image's
        // name and size instead.
        let Some(host) = docker_host(&ctx.home, &scan_docker_config)? else {
            return Ok(findings);
        };
        let version =
            docker(&ctx.home, &scan_docker_config, &host, &["version"]).map_err(|error| {
                let disclosed = findings
                    .iter()
                    .map(|finding| {
                        format!(
                            "; {} is {} on this host",
                            finding.label,
                            crate::report::gb(finding.size_bytes)
                        )
                    })
                    .collect::<String>();
                error.context(format!(
                "Docker daemon unreachable, so no image or build-cache plan was produced{disclosed}"
            ))
            })?;
        if version.trim().is_empty() {
            anyhow::bail!("`docker version` returned empty output");
        }
        let output = docker(
            &ctx.home,
            &scan_docker_config,
            &host,
            &[
                "system",
                "df",
                "--format",
                "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}\t{{.Active}}",
            ],
        )?;
        findings.extend(parse_system_df(&output, &host)?);
        findings.extend(scan_buildx_findings(&ctx.home, &scan_docker_config, &host)?);
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            // The host VM disk image is a report-only disclosure, not an action.
            // The skip is on ACTIONABILITY, never on whether an authority is
            // present: an actionable finding that carries no authority is a
            // forgery and must still be refused below.
            if !finding.action.is_actionable() {
                continue;
            }
            let result = (|| -> Result<String> {
                let Some(authority) = finding.command_authority() else {
                    anyhow::bail!("refusing unexpected Docker action");
                };
                if !matches!(
                    authority,
                    CommandAuthority::DockerImagePrune { .. }
                        | CommandAuthority::DockerBuilderPrune { .. }
                        | CommandAuthority::DockerBuildxPrune { .. }
                ) {
                    anyhow::bail!("refusing unexpected Docker action");
                }
                let host = authority
                    .docker_host()
                    .ok_or_else(|| anyhow::anyhow!("Docker action missing endpoint authority"))?;
                if !is_local_docker_host(host) {
                    anyhow::bail!("refusing non-local Docker endpoint `{host}`");
                }
                if finding.action != authority.action() {
                    anyhow::bail!("refusing altered Docker action");
                }
                if let CommandAuthority::DockerBuildxPrune {
                    host,
                    builder,
                    nodes,
                } = authority
                {
                    if !is_valid_builder_name(builder) {
                        anyhow::bail!("refusing invalid Docker buildx builder name `{builder}`");
                    }
                    if nodes.is_empty() {
                        anyhow::bail!(
                            "refusing buildx prune: builder `{builder}` carries no previewed node identity"
                        );
                    }
                    let scan_docker_config = isolated_docker_scan_dir(&ctx.home)?;
                    verify_local_buildx_builder(
                        &ctx.home,
                        &scan_docker_config,
                        host,
                        builder,
                        nodes,
                    )?;
                } else {
                    resolve_docker_cli_plugin_dirs(&ctx.home)?;
                }
                super::run_command_authority(
                    self.name(),
                    authority,
                    finding.size_bytes,
                    ctx,
                    |command| format!("{command} completed"),
                )
            })();
            match result {
                Ok(note) => outcome.record(finding, note),
                Err(error) => {
                    outcome.fail(error);
                    break;
                }
            }
        }
        if outcome.summary.items_touched > 0 {
            outcome
                .summary
                .notes
                .push("the host VM disk image shrinks only once the runtime returns freed blocks; scan again to measure it".into());
        }
        Ok(outcome)
    }
}

/// Size and effect of `builder prune -a` for one `docker system df` row.
///
/// The daemon sums every record into SIZE but counts a record as RECLAIMABLE
/// only when it is neither in use nor shared with the image store
/// (moby `daemon/internal/builder-next/builder.go`, `DiskUsage`). `-a` prunes
/// with BuildKit's `All`, which skips that shared check
/// (`cache/manager.go`, `prune`), so it removes shared records RECLAIMABLE left
/// out: one OrbStack machine showed RECLAIMABLE 4.902GB, SIZE 17.37GB, ACTIVE 0,
/// and the prune reported 17.37GB. With nothing in use SIZE is therefore the
/// ceiling; bytes an image still references stay on disk, so it is not a
/// promise. With records in use their share of SIZE is unknown, and
/// RECLAIMABLE is the floor.
fn build_cache_estimate(total: &str, reclaimable: &str, active: &str) -> Result<(u64, String)> {
    let active: u64 = active
        .parse()
        .with_context(|| format!("invalid Docker build cache active count: {active}"))?;
    if active == 0 {
        let bytes = parse_size(total)
            .with_context(|| format!("invalid Docker build cache size: {total}"))?;
        return Ok((
            bytes,
            "up to this much: no record is in use, and `builder prune -a` removes every record, \
             including cache shared with images that Docker does not count as reclaimable; \
             bytes an image still references stay on disk"
                .to_string(),
        ));
    }
    let bytes = parse_size(reclaimable)
        .with_context(|| format!("invalid Docker reclaimable size: {reclaimable}"))?;
    Ok((
        bytes,
        format!(
            "at least this much: {active} in-use records are kept, and `builder prune -a` also \
             removes unused cache shared with images, which this estimate leaves out"
        ),
    ))
}

pub(crate) fn parse_size(value: &str) -> Result<u64> {
    let value = value.split('(').next().unwrap_or("").trim();
    let split = value
        .find(|character: char| character.is_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().context("invalid numeric size")?;
    let multiplier = match unit.trim().to_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "pb" => 1e15,
        "eb" => 1e18,
        "kib" => 1024.0,
        "mib" => 1_048_576.0,
        "gib" => 1_073_741_824.0,
        "tib" => 1_099_511_627_776.0,
        "pib" => 1_125_899_906_842_624.0,
        "eib" => 1_152_921_504_606_846_976.0,
        unit => anyhow::bail!("unsupported size unit `{unit}`"),
    };
    let bytes = (number * multiplier).round();
    // `i64::MAX as f64` rounds up to 2^63, so equality is already ambiguous.
    if !bytes.is_finite() || bytes < 0.0 || bytes >= i64::MAX as f64 {
        anyhow::bail!("size is outside Docker's signed 64-bit byte range");
    }
    Ok(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::report::Action;
    use std::path::PathBuf;

    fn test_ctx() -> Ctx {
        Ctx {
            journal_path: PathBuf::from("/tmp/devtrim-docker-test-journal.jsonl"),
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            ..Ctx::for_test(PathBuf::from("/tmp"))
        }
    }

    /// Positive control for the measurement basis itself: a purely logical
    /// measurement passes every other assertion here, so without this the fix
    /// could regress to `len()` silently.
    #[test]
    fn allocated_bytes_measures_allocation_not_logical_length() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-docker-sparse")
            .tempdir()
            .unwrap();
        let path = root.path().join("data.img.raw");
        let file = std::fs::File::create(&path).unwrap();
        // A hole, not a write: logical length grows to 1 GiB while almost no
        // blocks are allocated. This is the exact shape of a VM disk image.
        file.set_len(1024 * 1024 * 1024).unwrap();
        file.sync_all().unwrap();

        let logical = std::fs::symlink_metadata(&path).unwrap().len();
        let allocated = allocated_bytes(&path).unwrap().unwrap();

        assert_eq!(logical, 1024 * 1024 * 1024);
        assert!(
            allocated < logical / 100,
            "allocated {allocated} should be far below logical {logical}"
        );
    }

    /// Apply's actionability skip must not become a hole: an actionable
    /// finding that carries no command authority is a forgery and stays refused.
    #[test]
    fn rejects_forged_actions_without_authority() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-docker-forged")
            .tempdir()
            .unwrap();
        let sentinel = home.path().join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let mut ctx = test_ctx();
        ctx.home = home.path().to_path_buf();
        ctx.journal_path = home.path().join("journal.jsonl");
        let pathless_command = Action::command("docker", &["image", "prune", "-a", "-f"]);
        for finding in [
            Finding::new(
                "forged docker action",
                Some(sentinel.clone()),
                1,
                "forged",
                6,
                Action::Trash,
            ),
            // Docker volumes are never pruned: keep the forged volume-prune payload explicit.
            Finding::new(
                "forged docker action",
                Some(sentinel.clone()),
                1,
                "forged",
                6,
                Action::command("docker", &["volume", "prune", "-f"]),
            ),
            Finding::new(
                "forged docker action",
                Some(sentinel.clone()),
                1,
                "forged",
                6,
                Action::command("docker", &["image", "prune", "-a", "-f"]),
            ),
            Finding::new("pathless forgery", None, 1, "forged", 6, pathless_command),
        ] {
            let outcome = Docker.apply(&[finding], &ctx).unwrap();
            assert_eq!(outcome.summary.items_touched, 0);
            assert_eq!(outcome.errors.len(), 1);
            assert!(
                outcome.errors[0].contains("refusing unexpected Docker action"),
                "{:?}",
                outcome.errors
            );
            assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        }
    }

    #[test]
    fn rejects_ambiguous_or_out_of_range_docker_sizes() {
        for value in [
            "5",
            "5XB",
            "-1GB",
            "NaNGB",
            "10EB",
            "9223372036854775807B",
            "9223372036854775808B",
        ] {
            assert!(parse_size(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn rejects_malformed_docker_system_df_output() {
        let host = "unix:///var/run/docker.sock";
        assert!(parse_system_df("", host).is_err());
        assert!(parse_system_df("Images\t1GB", host).is_err());
        assert!(parse_system_df("Images\t1GB\t500MB", host).is_err());
        assert!(parse_system_df("Images\t1GB\t500MB\t1\textra", host).is_err());
        assert!(parse_system_df("Build Cache\t1GB\t500MB\t", host).is_err());
        assert!(parse_system_df("Build Cache\t1GB\t500MB\tsome", host).is_err());
    }

    #[test]
    fn docker_context_must_resolve_to_an_absolute_local_socket() {
        let local = r#"[{"Endpoints":{"docker":{"Host":"unix:///Users/example/.docker/run/docker.sock"}}}]"#;
        assert_eq!(
            parse_docker_host(local).unwrap(),
            "unix:///Users/example/.docker/run/docker.sock"
        );

        for host in [
            "ssh://prod.example",
            "tcp://127.0.0.1:2375",
            "unix://relative.sock",
            "unix:///tmp/../remote.sock",
        ] {
            let document = format!(r#"[{{"Endpoints":{{"docker":{{"Host":"{host}"}}}}}}]"#);
            assert!(parse_docker_host(&document).is_err(), "accepted {host}");
        }
    }

    #[test]
    fn local_docker_contexts_keep_only_absolute_unix_sockets() {
        let output = concat!(
            r#"{"Name":"orbstack","DockerEndpoint":"unix:///Users/example/.orbstack/run/docker.sock"}"#,
            "\n",
            r#"{"Name":"prod","DockerEndpoint":"ssh://root@prod.example"}"#,
            "\n",
            r#"{"Name":"tcp","DockerEndpoint":"tcp://127.0.0.1:2375"}"#,
            "\n",
            r#"{"Name":"relative","DockerEndpoint":"unix://docker.sock"}"#,
            "\n"
        );
        let contexts = parse_local_docker_contexts(output).unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(
            contexts.get("orbstack").map(String::as_str),
            Some("unix:///Users/example/.orbstack/run/docker.sock")
        );
        assert!(parse_local_docker_contexts("not-json\n").is_err());
    }

    #[test]
    fn buildx_selection_deduplicates_aliases_and_rejects_non_local_or_non_container_builders() {
        let mut local_contexts = BTreeMap::new();
        local_contexts.insert(
            "orbstack".to_string(),
            "unix:///Users/example/.orbstack/run/docker.sock".to_string(),
        );
        let ndjson = concat!(
            // Duplicate rows for alias `kiri` and `kiri-build` sharing the same node ID;
            // `kiri-build` is Current == true so it must win over lexicographically smaller `kiri`.
            r#"{"Name":"kiri","Driver":"docker-container","Current":false,"Nodes":[{"Name":"kiri-build0","Endpoint":"orbstack","Status":"running","IDs":["zmazdby6v31ni8m0iz6hq923g"]}]}"#,
            "\n",
            r#"{"Name":"kiri","Driver":"docker-container","Current":false,"Nodes":[{"Name":"kiri-build0","Endpoint":"orbstack","Status":"running","IDs":["zmazdby6v31ni8m0iz6hq923g"]}]}"#,
            "\n",
            r#"{"Name":"kiri-build","Driver":"docker-container","Current":true,"Nodes":[{"Name":"kiri-build0","Endpoint":"orbstack","Status":"running","IDs":["zmazdby6v31ni8m0iz6hq923g"]}]}"#,
            "\n",
            // Two non-current aliases sharing `(resolved endpoint, ID)` even with different node names: smallest name `alpha` wins.
            r#"{"Name":"zeta","Driver":"docker-container","Current":false,"Nodes":[{"Name":"shared-a","Endpoint":"unix:///var/run/docker.sock","Status":"running","IDs":["id-shared"]}]}"#,
            "\n",
            r#"{"Name":"alpha","Driver":"docker-container","Current":false,"Nodes":[{"Name":"shared-b","Endpoint":"unix:///var/run/docker.sock","Status":"running","IDs":["id-shared"]}]}"#,
            "\n",
            // Delimiter safety: `["a,b"]` and `["a","b"]` are distinct worker IDs and must stay separate.
            r#"{"Name":"comma-one","Driver":"docker-container","Current":false,"Nodes":[{"Name":"c0","Endpoint":"unix:///var/run/docker.sock","Status":"running","IDs":["a,b"]}]}"#,
            "\n",
            r#"{"Name":"comma-two","Driver":"docker-container","Current":false,"Nodes":[{"Name":"c0","Endpoint":"unix:///var/run/docker.sock","Status":"running","IDs":["a","b"]}]}"#,
            "\n",
            // Rejected: missing worker IDs, non-container drivers, remote node, stopped node, malformed name.
            r#"{"Name":"no-ids","Driver":"docker-container","Current":false,"Nodes":[{"Name":"n0","Endpoint":"unix:///var/run/docker.sock","Status":"running","IDs":[]}]}"#,
            "\n",
            r#"{"Name":"default","Driver":"docker","Current":false,"Nodes":[{"Name":"default","Endpoint":"orbstack","Status":"running","IDs":null}]}"#,
            "\n",
            r#"{"Name":"remote-builder","Driver":"remote","Current":false,"Nodes":[{"Name":"r0","Endpoint":"tls://remote:1234","Status":"running","IDs":null}]}"#,
            "\n",
            r#"{"Name":"cloud-builder","Driver":"cloud","Current":false,"Nodes":[{"Name":"c0","Endpoint":"cloud://org/b","Status":"running","IDs":null}]}"#,
            "\n",
            r#"{"Name":"mixed-remote","Driver":"docker-container","Current":false,"Nodes":[{"Name":"n0","Endpoint":"orbstack","Status":"running","IDs":["id-local"]},{"Name":"n1","Endpoint":"prod-ssh","Status":"running","IDs":["id-remote"]}]}"#,
            "\n",
            r#"{"Name":"stopped-builder","Driver":"docker-container","Current":false,"Nodes":[{"Name":"s0","Endpoint":"orbstack","Status":"stopped","IDs":["id-stopped"]}]}"#,
            "\n",
            r#"{"Name":"-bad-name","Driver":"docker-container","Current":false,"Nodes":[{"Name":"b0","Endpoint":"orbstack","Status":"running","IDs":["id-bad"]}]}"#,
            "\n"
        );
        let parsed = parse_buildx_ls(ndjson).unwrap();
        let selected = deduplicate_local_buildx_builders(&parsed, &local_contexts);
        let names: Vec<&str> = selected
            .iter()
            .map(|builder| builder.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "comma-one", "comma-two", "kiri-build"]);
    }

    #[test]
    fn rejects_forged_buildx_actions_before_running_docker() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-docker-buildx-forged")
            .tempdir()
            .unwrap();
        let mut ctx = test_ctx();
        ctx.home = home.path().to_path_buf();
        ctx.journal_path = home.path().join("journal.jsonl");

        let sample_node = || {
            vec![BuildxNodeIdentity {
                name: "kiri-build0".into(),
                endpoint: "unix:///var/run/docker.sock".into(),
                ids: vec!["node-1".into()],
            }]
        };
        let non_local = Finding::command(
            "remote buildx",
            1024,
            "forged",
            6,
            CommandAuthority::DockerBuildxPrune {
                host: "ssh://prod.example".into(),
                builder: "kiri-build".into(),
                nodes: sample_node(),
            },
        );
        let outcome = Docker.apply(&[non_local], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert!(
            outcome.errors[0].contains("refusing non-local Docker endpoint"),
            "{:?}",
            outcome.errors
        );

        let bad_name = Finding::command(
            "bad builder",
            1024,
            "forged",
            6,
            CommandAuthority::DockerBuildxPrune {
                host: "unix:///var/run/docker.sock".into(),
                builder: "--all".into(),
                nodes: sample_node(),
            },
        );
        let outcome = Docker.apply(&[bad_name], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert!(
            outcome.errors[0].contains("refusing invalid Docker buildx builder name"),
            "{:?}",
            outcome.errors
        );

        let missing_nodes = Finding::command(
            "missing nodes",
            1024,
            "forged",
            6,
            CommandAuthority::DockerBuildxPrune {
                host: "unix:///var/run/docker.sock".into(),
                builder: "kiri-build".into(),
                nodes: Vec::new(),
            },
        );
        let outcome = Docker.apply(&[missing_nodes], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert!(
            outcome.errors[0].contains("carries no previewed node identity"),
            "{:?}",
            outcome.errors
        );
    }

    #[test]
    fn docker_config_validation_refuses_relative_traversal_and_symlinked_dirs() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-docker-config-validate")
            .tempdir()
            .unwrap();
        let home = root.path();
        let real_dir = home.join("real-docker");
        std::fs::create_dir_all(&real_dir).unwrap();
        let linked_dir = home.join("linked-docker");
        std::os::unix::fs::symlink(&real_dir, &linked_dir).unwrap();
        let regular_file = home.join("not-a-dir");
        std::fs::write(&regular_file, "{}").unwrap();

        for invalid in [
            std::ffi::OsStr::new(".docker"),
            std::ffi::OsStr::new("./.docker"),
            std::ffi::OsStr::new("/tmp/../.docker"),
            std::ffi::OsStr::new("/tmp/./.docker"),
            linked_dir.as_os_str(),
            regular_file.as_os_str(),
        ] {
            assert!(
                validate_docker_config_dir_from(Some(invalid), home).is_err(),
                "accepted invalid DOCKER_CONFIG {}",
                Path::new(invalid).display()
            );
        }

        assert_eq!(
            validate_docker_config_dir_from(Some(real_dir.as_os_str()), home).unwrap(),
            real_dir
        );
        // Missing default `~/.docker` stays allowed, while a symlinked or
        // non-directory `~/.docker` must be refused.
        assert_eq!(
            validate_docker_config_dir_from(None, home).unwrap(),
            home.join(".docker")
        );
        std::os::unix::fs::symlink(&real_dir, home.join(".docker")).unwrap();
        assert!(
            validate_docker_config_dir_from(None, home).is_err(),
            "symlinked default ~/.docker must be refused"
        );
        std::fs::remove_file(home.join(".docker")).unwrap();
        std::fs::write(home.join(".docker"), "{}").unwrap();
        assert!(
            validate_docker_config_dir_from(None, home).is_err(),
            "regular-file default ~/.docker must be refused"
        );
        std::fs::remove_file(home.join(".docker")).unwrap();
        std::fs::create_dir_all(home.join(".docker")).unwrap();
        assert_eq!(
            validate_docker_config_dir_from(None, home).unwrap(),
            home.join(".docker")
        );
    }

    #[test]
    fn docker_config_json_fails_closed_on_symlink_fifo_and_malformed_json() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-docker-config-json")
            .tempdir()
            .unwrap();
        let home = root.path();

        // Symlinked config.json fails closed (`O_NOFOLLOW`).
        let symlink_config = home.join("symlink-config");
        std::fs::create_dir_all(&symlink_config).unwrap();
        let target_file = home.join("external.json");
        std::fs::write(&target_file, "{}").unwrap();
        std::os::unix::fs::symlink(&target_file, symlink_config.join("config.json")).unwrap();
        assert!(validate_docker_config_from(Some(symlink_config.as_os_str()), home).is_err());

        // FIFO config.json fails closed without blocking (`O_NONBLOCK` + regular-file check).
        let fifo_config = home.join("fifo-config");
        std::fs::create_dir_all(&fifo_config).unwrap();
        let fifo_path = fifo_config.join("config.json");
        let status = Command::new("/usr/bin/mkfifo")
            .arg(&fifo_path)
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo failed");
        let error = validate_docker_config_from(Some(fifo_config.as_os_str()), home)
            .expect_err("FIFO config.json must fail closed");
        assert!(
            format!("{error:#}").contains("non-regular Docker config"),
            "{error:#}"
        );

        // Malformed JSON and non-object JSON fail closed.
        let bad_json_config = home.join("bad-json-config");
        std::fs::create_dir_all(&bad_json_config).unwrap();
        std::fs::write(bad_json_config.join("config.json"), "{not-json").unwrap();
        assert!(validate_docker_config_from(Some(bad_json_config.as_os_str()), home).is_err());
        std::fs::write(bad_json_config.join("config.json"), "[]").unwrap();
        assert!(validate_docker_config_from(Some(bad_json_config.as_os_str()), home).is_err());
    }

    #[test]
    fn docker_cli_plugin_dirs_refuses_unsafe_entries_and_keeps_safe_ones() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::Builder::new()
            .prefix("devtrim-docker-plugin-dirs")
            .tempdir()
            .unwrap();
        let home = root.path();
        let safe_plugins = home.join("safe-plugins");
        std::fs::create_dir_all(&safe_plugins).unwrap();
        std::fs::set_permissions(&safe_plugins, std::fs::Permissions::from_mode(0o755)).unwrap();

        let writable_plugins = home.join("writable-plugins");
        std::fs::create_dir_all(&writable_plugins).unwrap();
        std::fs::set_permissions(&writable_plugins, std::fs::Permissions::from_mode(0o777))
            .unwrap();

        let source_config = home.join("docker-config");
        std::fs::create_dir_all(&source_config).unwrap();

        // Relative, traversal, and world-writable plugin directories fail closed.
        for bad_entry in [
            "./plugins".to_string(),
            "relative/dir".to_string(),
            "/usr/local/../opt/plugins".to_string(),
            writable_plugins.to_string_lossy().into_owned(),
        ] {
            std::fs::write(
                source_config.join("config.json"),
                serde_json::json!({ "clipluginsextradirs": [bad_entry] }).to_string(),
            )
            .unwrap();
            assert!(
                validate_docker_config_from(Some(source_config.as_os_str()), home).is_err(),
                "must refuse unsafe cliPluginsExtraDirs entry: {bad_entry}"
            );
        }

        // Safe absolute 0755 owned plugin directories (and nonexistent absolute ones) pass.
        std::fs::write(
            source_config.join("config.json"),
            serde_json::json!({
                "clipluginsextradirs": [
                    safe_plugins.to_string_lossy(),
                    "/nonexistent/devtrim-test-docker-plugins"
                ]
            })
            .to_string(),
        )
        .unwrap();
        let plugin_dirs = validate_docker_config_from(Some(source_config.as_os_str()), home)
            .unwrap()
            .plugin_dirs;
        assert_eq!(
            plugin_dirs,
            vec![
                safe_plugins.to_string_lossy().into_owned(),
                "/nonexistent/devtrim-test-docker-plugins".to_string(),
                source_config
                    .join("cli-plugins")
                    .to_string_lossy()
                    .into_owned(),
            ]
        );

        // Writable ancestor above an otherwise 0755 plugin directory fails closed.
        let writable_parent = home.join("writable-parent");
        let nested_plugins = writable_parent.join("plugins");
        std::fs::create_dir_all(&nested_plugins).unwrap();
        std::fs::set_permissions(&writable_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(&nested_plugins, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            source_config.join("config.json"),
            serde_json::json!({ "cliPluginsExtraDirs": [nested_plugins.to_string_lossy()] })
                .to_string(),
        )
        .unwrap();
        assert!(
            validate_docker_config_from(Some(source_config.as_os_str()), home).is_err(),
            "plugin dir under writable ancestor must fail closed"
        );
        std::fs::set_permissions(&writable_parent, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Symlinked docker-* plugin pointing to a group/world-writable executable fails closed.
        let writable_plugin_target = home.join("writable-plugin-bin");
        std::fs::write(&writable_plugin_target, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            &writable_plugin_target,
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        let linked_plugin = safe_plugins.join("docker-buildx");
        std::os::unix::fs::symlink(&writable_plugin_target, &linked_plugin).unwrap();
        std::fs::write(
            source_config.join("config.json"),
            serde_json::json!({ "cliPluginsExtraDirs": [safe_plugins.to_string_lossy()] })
                .to_string(),
        )
        .unwrap();
        assert!(
            validate_docker_config_from(Some(source_config.as_os_str()), home).is_err(),
            "symlinked docker-* plugin pointing to writable executable must fail closed"
        );
        std::fs::remove_file(&linked_plugin).unwrap();

        // Group/world-writable config.json (even owned by current user) fails closed.
        std::fs::write(source_config.join("config.json"), "{}").unwrap();
        std::fs::set_permissions(
            source_config.join("config.json"),
            std::fs::Permissions::from_mode(0o664),
        )
        .unwrap();
        assert!(
            validate_docker_config_from(Some(source_config.as_os_str()), home).is_err(),
            "group-writable config.json must fail closed"
        );
        std::fs::set_permissions(
            source_config.join("config.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        // Group/world-writable DOCKER_CONFIG directory fails closed.
        std::fs::set_permissions(&source_config, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(
            validate_docker_config_from(Some(source_config.as_os_str()), home).is_err(),
            "group-writable DOCKER_CONFIG dir must fail closed"
        );
        std::fs::set_permissions(&source_config, std::fs::Permissions::from_mode(0o700)).unwrap();

        // Isolated scan snapshot strips aliases/extra keys and preserves currentContext + safe plugins.
        std::fs::write(
            source_config.join("config.json"),
            serde_json::json!({
                "currentContext": "orbstack",
                "aliases": { "builder": "buildx" },
                "cliPluginsExtraDirs": [safe_plugins.to_string_lossy()]
            })
            .to_string(),
        )
        .unwrap();
        let scan_base = home.join("scan-tmp");
        std::fs::create_dir_all(&scan_base).unwrap();
        std::fs::set_permissions(&scan_base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let scan_dir =
            isolated_docker_scan_dir_from(Some(source_config.as_os_str()), home, &scan_base)
                .unwrap();
        let scan_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(scan_dir.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(scan_json["currentContext"], "orbstack");
        assert!(scan_json.get("aliases").is_none());
        assert_eq!(
            scan_json["cliPluginsExtraDirs"],
            serde_json::json!([
                safe_plugins.to_string_lossy(),
                source_config.join("cli-plugins").to_string_lossy()
            ])
        );
    }

    #[test]
    fn private_docker_config_guard_removes_attempt_dir_on_drop() {
        let fixture = crate::ops::TestFixture::new("devtrim-docker-pin-cleanup");
        std::fs::create_dir_all(fixture.path()).unwrap();
        let home = fixture.path().canonicalize().unwrap();
        let state_dir = home.join(".local/state/devtrim");
        std::fs::create_dir_all(&state_dir).unwrap();
        let mut ctx = test_ctx();
        ctx.home = home.clone();
        ctx.journal_path = state_dir.join("journal.jsonl");

        let plugin_dirs = vec!["/opt/homebrew/lib/docker/cli-plugins".to_string()];
        let isolated_path = {
            let attempt = allocate_private_docker_config(&ctx, &plugin_dirs).unwrap();
            let path = attempt.guard.path().to_path_buf();
            assert!(path.join("config.json").is_file());
            assert!(path.join("buildx").is_dir());
            path
        };
        assert!(
            !isolated_path.exists(),
            "isolated attempt dir {} should be cleaned up on drop",
            isolated_path.display()
        );

        let nodes = vec![BuildxNodeIdentity {
            name: "kiri0".into(),
            endpoint: "unix:///tmp/kiri.sock".into(),
            ids: vec!["node-1".into()],
        }];
        let pinned_path = {
            let guard = write_pinned_buildx_config(&ctx, "kiri", &nodes, &plugin_dirs).unwrap();
            let path = guard.path().to_path_buf();
            assert!(path.join("buildx/instances/kiri").is_file());
            path
        };
        assert!(
            !pinned_path.exists(),
            "pinned attempt dir {} should be cleaned up on drop",
            pinned_path.display()
        );
        crate::ops::remove_test_path(home);
    }
}
