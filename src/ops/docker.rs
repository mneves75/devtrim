//! Docker/OrbStack pruning: unused images + build cache. Volumes are never pruned.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path};
use std::process::Command;

use super::{Action, ApplyOutcome, Finding, Op, command_stdout, optional_command_stdout};
use crate::process::{BoundedCommand as _, QUERY_TIMEOUT};
use crate::report::CommandAuthority;
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

fn docker(host: &str, args: &[&str]) -> Result<String> {
    let command = format!("`docker --host {host} {}`", args.join(" "));
    command_stdout(
        Command::new("docker")
            .arg("--host")
            .arg(host)
            .args(args)
            .output_within(QUERY_TIMEOUT),
        &command,
    )
}

fn docker_host() -> Result<Option<String>> {
    let Some(output) = optional_command_stdout(
        Command::new("docker")
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

fn local_docker_contexts() -> Result<BTreeMap<String, String>> {
    let output = command_stdout(
        Command::new("docker")
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

fn buildx_ls(host: &str) -> Result<Option<String>> {
    let output = Command::new("docker")
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
/// socket endpoint, and sorted worker IDs so neither an un-ID'd node nor a
/// retargeted context alias can slip past apply re-verification.
fn builder_node_fingerprint(
    nodes: &[BuildxNode],
    local_contexts: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut entries: Vec<String> = nodes
        .iter()
        .map(|node| {
            let endpoint = node.endpoint.trim();
            let resolved = local_contexts
                .get(endpoint)
                .map(String::as_str)
                .unwrap_or(endpoint);
            let mut node_ids: Vec<&str> = node
                .ids
                .iter()
                .flatten()
                .map(String::as_str)
                .map(str::trim)
                .filter(|identifier| !identifier.is_empty())
                .collect();
            node_ids.sort_unstable();
            node_ids.dedup();
            format!("{}@{}#{}", node.name.trim(), resolved, node_ids.join(","))
        })
        .collect();
    entries.sort();
    entries.dedup();
    entries
}

/// Filter to running local `docker-container` builders and collapse aliases
/// pointing at the exact same backing BuildKit nodes, preferring `Current == true`
/// then lexicographically smallest builder name.
fn deduplicate_local_buildx_builders(
    builders: &[BuildxBuilder],
    local_contexts: &BTreeMap<String, String>,
) -> Vec<BuildxBuilder> {
    let mut winners: BTreeMap<Vec<String>, BuildxBuilder> = BTreeMap::new();
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
        let key = builder_node_fingerprint(nodes, local_contexts);
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
        let all_local = !nodes.is_empty()
            && nodes.iter().all(|node| {
                let endpoint = node.endpoint.trim();
                is_local_docker_host(endpoint) || local_contexts.contains_key(endpoint)
            });
        if !all_local {
            continue;
        }
        for node in nodes {
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

fn scan_buildx_findings(host: &str) -> Result<Vec<Finding>> {
    let Some(output) = buildx_ls(host)? else {
        return Ok(Vec::new());
    };
    let parsed = parse_buildx_ls(&output)?;
    let local_contexts = if needs_context_lookup(&parsed) {
        local_docker_contexts()?
    } else {
        BTreeMap::new()
    };
    if let Some(error) = local_buildx_probe_error(&parsed, &local_contexts) {
        anyhow::bail!("{error}");
    }
    let builders = deduplicate_local_buildx_builders(&parsed, &local_contexts);
    let mut findings = Vec::new();
    for builder in builders {
        let du_output = docker(host, &["buildx", "du", "--builder", &builder.name])?;
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

fn verify_local_buildx_builder(host: &str, builder: &str, expected_nodes: &[String]) -> Result<()> {
    if expected_nodes.is_empty() {
        anyhow::bail!(
            "refusing buildx prune: builder `{builder}` carries no previewed node identity"
        );
    }
    let Some(output) = buildx_ls(host)? else {
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
        local_docker_contexts()?
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

impl Op for Docker {
    fn name(&self) -> &'static str {
        "docker"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        // The host image is disclosed even when the daemon is unreachable. A
        // stopped VM is precisely when its disk image is invisible to `docker`
        // and still occupying the host, so making this disclosure depend on a
        // live daemon would hide the cost in the one state that matters most.
        let mut findings = vm_disk_findings(&ctx.home)?;
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
        let Some(host) = docker_host()? else {
            return Ok(findings);
        };
        let version = docker(&host, &["version"]).map_err(|error| {
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
            &host,
            &[
                "system",
                "df",
                "--format",
                "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}\t{{.Active}}",
            ],
        )?;
        findings.extend(parse_system_df(&output, &host)?);
        findings.extend(scan_buildx_findings(&host)?);
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
                    verify_local_buildx_builder(host, builder, nodes)?;
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
            // Two non-current aliases sharing `(name, endpoint, ID)`: smallest name `alpha` wins.
            r#"{"Name":"zeta","Driver":"docker-container","Current":false,"Nodes":[{"Name":"shared0","Endpoint":"unix:///var/run/docker.sock","Status":"running","IDs":["id-shared"]}]}"#,
            "\n",
            r#"{"Name":"alpha","Driver":"docker-container","Current":false,"Nodes":[{"Name":"shared0","Endpoint":"unix:///var/run/docker.sock","Status":"running","IDs":["id-shared"]}]}"#,
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
        assert_eq!(names, vec!["alpha", "kiri-build"]);
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

        let non_local = Finding::command(
            "remote buildx",
            1024,
            "forged",
            6,
            CommandAuthority::DockerBuildxPrune {
                host: "ssh://prod.example".into(),
                builder: "kiri-build".into(),
                nodes: vec!["id:node-1".into()],
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
                nodes: vec!["id:node-1".into()],
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
}
