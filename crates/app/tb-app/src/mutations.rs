//! Complete mutation and recovery/preview use cases (`tb_preview_mutation`,
//! `tb_reconcile`, `tb_rollback_preview`, `tb_watch`, and the 11 structural,
//! composition, and workspace-shape mutations).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tb_contracts::{Locator, RevisionDigest, WriteGuardHash};
use tb_syntax::{Anchor, parse_document};

use crate::{App, AppError, IdempotentOutcome, LinkResolution, WorkspaceStore};

/// Target block selection for `tb_assign_block_id`.
#[derive(Clone, Copy, Debug)]
pub enum BlockTarget<'a> {
    Line(usize),
    MatchText(&'a str),
    Heading(&'a str),
    LastLine,
}

/// Source selection for `tb_promote`.
#[derive(Clone, Copy, Debug)]
pub enum PromoteSource<'a> {
    BlockId(&'a str),
    Heading(&'a str),
}

/// Operation on `workspace.yaml` membership (`--allow-workspace-shape`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkspaceMemberAction {
    Add {
        name: String,
        path: String,
        role: String,
    },
    Remove {
        name: String,
    },
    SetRole {
        name: String,
        role: String,
    },
}

/// Validate a `^block-id` slug (`1..=64` ASCII alphanumeric, `-`, `_`).
pub fn validate_block_id(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim().strip_prefix('^').unwrap_or(raw.trim());
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(AppError::InvalidArguments(
            "block_id must be 1..=64 characters".into(),
        ));
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppError::InvalidArguments(format!(
            "invalid block_id '{trimmed}': only ASCII letters, digits, '-' and '_' are allowed"
        )));
    }
    Ok(trimmed)
}

/// Validate a type or repository/member identifier slug.
pub fn validate_slug(label: &str, raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 96 {
        return Err(AppError::InvalidArguments(format!(
            "{label} must be 1..=96 characters"
        )));
    }
    if trimmed.starts_with('.') || trimmed.contains('/') || trimmed.contains('\\') {
        return Err(AppError::InvalidArguments(format!(
            "invalid {label} '{trimmed}'"
        )));
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(AppError::InvalidArguments(format!(
            "invalid {label} '{trimmed}': only ASCII alphanumeric, '-', '_', '.' allowed"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Validate a workspace member role (`own`, `dependency`, `primary`).
pub fn validate_member_role(raw: &str) -> Result<String, AppError> {
    let lower = raw.trim().to_ascii_lowercase();
    match lower.as_str() {
        "own" | "dependency" | "primary" => Ok(lower),
        _ => Err(AppError::InvalidArguments(format!(
            "invalid member role '{raw}': expected 'own', 'dependency', or 'primary'"
        ))),
    }
}

impl<W: WorkspaceStore + 'static> App<W> {
    /// `tb_preview_mutation`: dry-run a planned mutation purely in memory and return
    /// precondition status, post-edit diagnostics, and inbound-reference impact.
    pub fn preview_mutation(&self, args: &Map<String, Value>) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let op = args
            .get("op")
            .or_else(|| args.get("operation"))
            .and_then(Value::as_str)
            .unwrap_or("write_file");
        let loc = self.parse_locator_arg(args)?;
        let existing = snap.files.get(&loc);

        let mut issues: Vec<String> = Vec::new();
        let bytes_before = existing.map_or(0, |f| f.size_bytes);
        let mut candidate_content: Option<String> = None;

        match op {
            "create_file" => {
                if existing.is_some() {
                    issues.push(format!("already_exists: {}/{}", loc.repo, loc.path));
                }
                if let Some(c) = args.get("content").and_then(Value::as_str) {
                    candidate_content = Some(c.to_owned());
                } else {
                    issues.push("'content' is required for create_file".to_owned());
                }
            }
            "write_file" => {
                let Some(file) = existing else {
                    issues.push(format!("not_found: {}/{}", loc.repo, loc.path));
                    return Ok(json!({
                        "preview_mutation": {
                            "op": op,
                            "repo": loc.repo,
                            "path": loc.path,
                            "would_succeed": false,
                            "precondition_issues": issues,
                            "generation": snap.generation.0,
                        }
                    }));
                };
                if let Some(exp) = args.get("expected_hash").and_then(Value::as_str) {
                    let norm = exp.strip_prefix("sha256:").unwrap_or(exp);
                    if norm != file.short_hash && !norm.eq_ignore_ascii_case(&file.sha256.to_hex())
                    {
                        issues.push(format!(
                            "expected_hash_mismatch: expected {exp}, snapshot has {}",
                            file.short_hash
                        ));
                    }
                }
                if let Some(c) = args.get("content").and_then(Value::as_str) {
                    candidate_content = Some(c.to_owned());
                } else {
                    issues.push("'content' is required for write_file".to_owned());
                }
            }
            "edit_file" => {
                let Some(file) = existing else {
                    issues.push(format!("not_found: {}/{}", loc.repo, loc.path));
                    return Ok(json!({
                        "preview_mutation": {
                            "op": op,
                            "repo": loc.repo,
                            "path": loc.path,
                            "would_succeed": false,
                            "precondition_issues": issues,
                            "generation": snap.generation.0,
                        }
                    }));
                };
                if let Some(exp) = args.get("expected_hash").and_then(Value::as_str) {
                    let norm = exp.strip_prefix("sha256:").unwrap_or(exp);
                    if norm != file.short_hash && !norm.eq_ignore_ascii_case(&file.sha256.to_hex())
                    {
                        issues.push(format!(
                            "expected_hash_mismatch: expected {exp}, snapshot has {}",
                            file.short_hash
                        ));
                    }
                }
                let old_str = args
                    .get("old_str")
                    .or_else(|| args.get("old_string"))
                    .and_then(Value::as_str);
                let new_str = args
                    .get("new_str")
                    .or_else(|| args.get("new_string"))
                    .and_then(Value::as_str);
                match (old_str, new_str) {
                    (Some(old_s), Some(new_s)) => {
                        let matches = file.raw_text.matches(old_s).count();
                        if matches == 0 {
                            issues.push("old_str not found in file".to_owned());
                        } else if matches > 1 {
                            issues.push(format!("old_str is ambiguous ({matches} occurrences)"));
                        } else {
                            candidate_content = Some(file.raw_text.replacen(old_s, new_s, 1));
                        }
                    }
                    _ => issues.push("'old_str' and 'new_str' are required for edit_file".into()),
                }
            }
            "delete_file" | "rename" => {
                if let Some(file) = existing {
                    if let Some(exp) = args.get("expected_hash").and_then(Value::as_str) {
                        let norm = exp.strip_prefix("sha256:").unwrap_or(exp);
                        if norm != file.short_hash
                            && !norm.eq_ignore_ascii_case(&file.sha256.to_hex())
                        {
                            issues.push(format!(
                                "expected_hash_mismatch: expected {exp}, snapshot has {}",
                                file.short_hash
                            ));
                        }
                    }
                    if op == "rename" {
                        if let Some(new_path) = args
                            .get("new_path")
                            .or_else(|| args.get("to"))
                            .and_then(Value::as_str)
                        {
                            match Locator::new(&loc.repo, new_path) {
                                Ok(dst) => {
                                    if snap.files.contains_key(&dst) {
                                        issues.push(format!(
                                            "already_exists: {}/{}",
                                            dst.repo, dst.path
                                        ));
                                    }
                                }
                                Err(e) => issues.push(format!("invalid target path: {e}")),
                            }
                        } else {
                            issues.push("'new_path' is required for rename".to_owned());
                        }
                    }
                } else {
                    issues.push(format!("not_found: {}/{}", loc.repo, loc.path));
                }
            }
            other => {
                issues.push(format!("unsupported preview op '{other}'"));
            }
        }

        let inbound_affected: Vec<Value> = snap
            .graph
            .node_id(&loc)
            .map(|id| {
                snap.graph
                    .in_neighbors(id)
                    .0
                    .iter()
                    .filter_map(|&src| snap.graph.locator(src))
                    .map(|l| json!({ "repo": l.repo, "path": l.path }))
                    .collect()
            })
            .unwrap_or_default();

        let (bytes_after, after_diagnostics, next_hash) =
            if let Some(ref content) = candidate_content {
                let bytes = content.as_bytes();
                let diags = match parse_document(bytes) {
                    Ok(doc) => snap.registry.validate_document(&loc.repo, &loc.path, &doc),
                    Err(_) => vec![],
                };
                let h = WriteGuardHash::of_bytes(bytes).to_hex();
                (bytes.len(), diags, Some(h))
            } else {
                (0, vec![], None)
            };

        Ok(json!({
            "preview_mutation": {
                "op": op,
                "repo": loc.repo,
                "path": loc.path,
                "would_succeed": issues.is_empty(),
                "precondition_issues": issues,
                "bytes_before": bytes_before,
                "bytes_after": bytes_after,
                "next_hash": next_hash,
                "after_diagnostics": after_diagnostics,
                "inbound_references_affected": inbound_affected,
                "generation": snap.generation.0,
            }
        }))
    }

    /// `tb_reconcile`: inspect live disk bytes vs snapshot state and optional outcome journal
    /// record so callers can recover honestly from `outcome_unknown`.
    pub fn reconcile(
        &self,
        locator: &Locator,
        expected_hash: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let snap_file = snap.files.get(locator);
        let outcome = match request_id {
            Some(rid) => Some(self.outcomes.observe(rid)?),
            None => None,
        };

        match self.store.read_exact(locator) {
            Ok((bytes, receipt)) => {
                let disk_hash = receipt.write_guard.to_hex();
                let disk_sha256 = receipt.sha256.to_hex();
                let snapshot_hash = snap_file.map(|f| f.short_hash.clone());
                let matches_snapshot = snapshot_hash.as_deref() == Some(disk_hash.as_str());
                let matches_expected = expected_hash.map(|exp| {
                    let norm = exp.strip_prefix("sha256:").unwrap_or(exp);
                    norm == disk_hash || norm.eq_ignore_ascii_case(&disk_sha256)
                });
                let status = if matches_snapshot {
                    "matched"
                } else {
                    "drifted"
                };
                Ok(json!({
                    "reconcile": {
                        "repo": locator.repo,
                        "path": locator.path,
                        "status": status,
                        "exists_on_disk": true,
                        "exists_in_snapshot": snap_file.is_some(),
                        "disk_hash": disk_hash,
                        "disk_sha256": disk_sha256,
                        "disk_bytes": bytes.len(),
                        "snapshot_hash": snapshot_hash,
                        "matches_expected": matches_expected,
                        "outcome": outcome,
                        "generation": snap.generation.0,
                    }
                }))
            }
            Err(crate::StoreError::NotFound(_)) => Ok(json!({
                "reconcile": {
                    "repo": locator.repo,
                    "path": locator.path,
                    "status": "missing_on_disk",
                    "exists_on_disk": false,
                    "exists_in_snapshot": snap_file.is_some(),
                    "snapshot_hash": snap_file.map(|f| f.short_hash.clone()),
                    "matches_expected": expected_hash.map(|_| false),
                    "outcome": outcome,
                    "generation": snap.generation.0,
                }
            })),
            Err(e) => Err(e.into()),
        }
    }

    /// `tb_rollback_preview`: inspect files affected by rolling back a Git commit (default `HEAD`).
    pub fn rollback_preview(
        &self,
        repo: Option<&str>,
        commit_ref: &str,
        limit: usize,
    ) -> Result<Value, AppError> {
        let repo_name = repo
            .map(str::to_owned)
            .or_else(|| self.snapshot().members.keys().next().cloned())
            .ok_or_else(|| AppError::NotFound("no workspace member".into()))?;
        let preview = self
            .store
            .git_commit_preview(&repo_name, commit_ref, limit.clamp(1, 200))?;
        Ok(json!({
            "rollback_preview": preview,
            "generation": self.generation().0,
        }))
    }

    /// `tb_watch`: block until the workspace snapshot generation advances beyond
    /// `since_generation`, or until `timeout_ms` elapses (capped at 30 seconds).
    pub fn watch(&self, since_generation: Option<u64>, timeout_ms: u64) -> Result<Value, AppError> {
        let target = since_generation.unwrap_or_else(|| self.generation().0);
        let timeout = Duration::from_millis(timeout_ms.clamp(10, 30_000));
        let deadline = Instant::now() + timeout;

        let (lock, cvar) = &self.watch_notify;
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *guard <= target {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let wait_for = deadline.saturating_duration_since(now);
            let (next_guard, res) = cvar
                .wait_timeout(guard, wait_for)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard = next_guard;
            if res.timed_out() {
                break;
            }
        }
        let current = *guard;
        Ok(json!({
            "watch": {
                "changed": current > target,
                "since_generation": target,
                "generation": current,
            }
        }))
    }

    /// `tb_assign_block_id`: append a `^block-id` anchor to a line, heading, or matching block.
    pub fn assign_block_id(
        &self,
        locator: &Locator,
        expected_hash: &str,
        block_id_raw: &str,
        target: BlockTarget<'_>,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let block_id = validate_block_id(block_id_raw)?.to_owned();
        let snap = self.snapshot();
        let file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        if file
            .doc
            .anchors
            .iter()
            .any(|a| matches!(a, Anchor::Block { id, .. } if id == &block_id))
        {
            return Err(AppError::AlreadyExists(format!(
                "block id '^{block_id}' already exists in {}/{}",
                locator.repo, locator.path
            )));
        }

        let (line, match_text, heading) = match target {
            BlockTarget::Line(n) => (Some(n), None, None),
            BlockTarget::MatchText(t) => (None, Some(t), None),
            BlockTarget::Heading(h) => (None, None, Some(h)),
            BlockTarget::LastLine => (None, None, None),
        };
        let updated =
            insert_block_id_into_content(&file.raw_text, &block_id, line, match_text, heading)?;
        let commit_msg = format!("2brain: assign ^{block_id} in {}", locator.path);
        let run = || {
            let receipt =
                self.store
                    .write_file_cas(locator, expected_hash, &updated, &commit_msg)?;
            self.refresh_after_mutation();
            Ok(json!({
                "assign_block_id": {
                    "repo": locator.repo,
                    "path": locator.path,
                    "block_id": block_id,
                    "receipt": receipt,
                }
            }))
        };
        if let Some(req_id) = request_id {
            let args = json!({
                "repo": locator.repo,
                "path": locator.path,
                "block_id": block_id,
                "line": line,
                "match_text": match_text,
                "heading": heading,
            });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(
                req_id,
                "assign_block_id",
                &locator.repo,
                &args,
                &pre,
                run,
            );
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_rename_block_id`: rename a `^block-id` in the defining note AND update all
    /// inbound wikilinks across the workspace referencing `^old_id`.
    pub fn rename_block_id(
        &self,
        locator: &Locator,
        expected_hash: &str,
        old_id_raw: &str,
        new_id_raw: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let old_id = validate_block_id(old_id_raw)?.to_owned();
        let new_id = validate_block_id(new_id_raw)?.to_owned();
        if old_id == new_id {
            return Err(AppError::InvalidArguments(
                "old_id and new_id must differ".into(),
            ));
        }

        let snap = self.snapshot();
        let file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        if !file
            .doc
            .anchors
            .iter()
            .any(|a| matches!(a, Anchor::Block { id, .. } if id == &old_id))
        {
            return Err(AppError::NotFound(format!(
                "block id '^{old_id}' not found in {}/{}",
                locator.repo, locator.path
            )));
        }
        if file
            .doc
            .anchors
            .iter()
            .any(|a| matches!(a, Anchor::Block { id, .. } if id == &new_id))
        {
            return Err(AppError::AlreadyExists(format!(
                "block id '^{new_id}' already exists in {}/{}",
                locator.repo, locator.path
            )));
        }

        let old_marker = format!("^{old_id}");
        let new_marker = format!("^{new_id}");
        let updated_owner = replace_block_id_definition(&file.raw_text, &old_marker, &new_marker)?;

        // Collect ONLY exact parsed wikilink byte spans whose resolved target is `locator`
        // AND whose anchor matches `^old_id` (never touching links to other notes).
        let mut inbound_updates: Vec<(Locator, String, String)> = Vec::new();
        for other in snap.files.values() {
            if &other.locator == locator {
                continue;
            }
            let matching_spans: Vec<(usize, usize)> = other
                .doc
                .wikilinks
                .iter()
                .filter(|wl| {
                    wl.anchor.as_deref().and_then(|a| a.strip_prefix('^')) == Some(old_id.as_str())
                        && matches!(
                            snap.resolve_link(&other.locator.repo, &wl.target, wl.repo.as_deref()),
                            LinkResolution::Resolved(ref target_loc) if target_loc == locator
                        )
                })
                .map(|wl| (wl.byte_start, wl.byte_end))
                .collect();
            if !matching_spans.is_empty() {
                let rewritten =
                    rewrite_block_id_spans(&other.raw_text, &matching_spans, &old_id, &new_id);
                if rewritten != other.raw_text {
                    inbound_updates.push((
                        other.locator.clone(),
                        other.short_hash.clone(),
                        rewritten,
                    ));
                }
            }
        }

        let commit_msg = format!("2brain: rename ^{old_id} -> ^{new_id} in {}", locator.path);
        let run = || {
            let receipt =
                self.store
                    .write_file_cas(locator, expected_hash, &updated_owner, &commit_msg)?;
            let mut updated_refs = Vec::new();
            let mut failed_refs = Vec::new();
            for (ref_loc, ref_hash, ref_content) in &inbound_updates {
                let ref_msg = format!(
                    "2brain: update ^{old_id} -> ^{new_id} ref in {}",
                    ref_loc.path
                );
                match self
                    .store
                    .write_file_cas(ref_loc, ref_hash, ref_content, &ref_msg)
                {
                    Ok(_) => {
                        updated_refs.push(json!({ "repo": ref_loc.repo, "path": ref_loc.path }));
                    }
                    Err(err) => {
                        failed_refs.push(format!("{}/{}: {err}", ref_loc.repo, ref_loc.path));
                    }
                }
            }
            self.refresh_after_mutation();
            if !failed_refs.is_empty() {
                return Err(AppError::OutcomeUnknown(format!(
                    "owner '{}/{}' updated ({}), but {} dependent reference update(s) failed: [{}]",
                    locator.repo,
                    locator.path,
                    receipt.hash,
                    failed_refs.len(),
                    failed_refs.join("; ")
                )));
            }
            Ok(json!({
                "rename_block_id": {
                    "repo": locator.repo,
                    "path": locator.path,
                    "old_id": old_id,
                    "new_id": new_id,
                    "receipt": receipt,
                    "updated_references": updated_refs,
                }
            }))
        };
        if let Some(req_id) = request_id {
            let args = json!({
                "repo": locator.repo,
                "path": locator.path,
                "old_id": old_id,
                "new_id": new_id,
            });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(
                req_id,
                "rename_block_id",
                &locator.repo,
                &args,
                &pre,
                run,
            );
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_rename_type`: rename `type/<old>.type.yaml` -> `type/<new>.type.yaml`,
    /// update `type:` frontmatter in all instances, and update `extends:` in subtypes.
    pub fn rename_type(
        &self,
        repo: &str,
        old_name_raw: &str,
        new_name_raw: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let old_name = validate_slug("old_name", old_name_raw)?;
        let new_name = validate_slug("new_name", new_name_raw)?;
        if old_name == new_name {
            return Err(AppError::InvalidArguments(
                "old_name and new_name must differ".into(),
            ));
        }

        let snap = self.snapshot();
        if !snap
            .registry
            .iter()
            .any(|t| t.repo == repo && t.name == old_name)
        {
            return Err(AppError::NotFound(format!("type '{old_name}' in '{repo}'")));
        }
        if snap
            .registry
            .iter()
            .any(|t| t.repo == repo && t.name == new_name)
        {
            return Err(AppError::AlreadyExists(format!(
                "type '{new_name}' already exists in '{repo}'"
            )));
        }

        let from_type = Locator::new(repo, format!("type/{old_name}.type.yaml"))
            .map_err(|e| AppError::InvalidArguments(e.to_string()))?;
        let to_type = Locator::new(repo, format!("type/{new_name}.type.yaml"))
            .map_err(|e| AppError::InvalidArguments(e.to_string()))?;
        let (_, type_receipt) = self.store.read_exact(&from_type)?;
        let type_guard = type_receipt.write_guard.to_hex();

        let mut instance_updates: Vec<(Locator, String, String)> = Vec::new();
        for f in snap.files.values() {
            if f.locator.repo == repo && f.declared_type.as_deref() == Some(old_name.as_str()) {
                let updated =
                    replace_frontmatter_field_value(&f.raw_text, "type", &old_name, &new_name);
                if updated != f.raw_text {
                    instance_updates.push((f.locator.clone(), f.short_hash.clone(), updated));
                }
            }
        }

        let commit_msg = format!("2brain: rename type {old_name} -> {new_name}");
        let run = || {
            let receipt =
                self.store
                    .rename_file_cas(&from_type, &to_type, &type_guard, &commit_msg)?;
            let mut updated_instances = Vec::new();
            let mut failed_instances = Vec::new();
            for (inst_loc, inst_hash, inst_content) in &instance_updates {
                let inst_msg = format!(
                    "2brain: update type {old_name} -> {new_name} in {}",
                    inst_loc.path
                );
                match self
                    .store
                    .write_file_cas(inst_loc, inst_hash, inst_content, &inst_msg)
                {
                    Ok(_) => {
                        updated_instances
                            .push(json!({ "repo": inst_loc.repo, "path": inst_loc.path }));
                    }
                    Err(err) => {
                        failed_instances
                            .push(format!("{}/{}: {err}", inst_loc.repo, inst_loc.path));
                    }
                }
            }
            self.refresh_after_mutation();
            if !failed_instances.is_empty() {
                return Err(AppError::OutcomeUnknown(format!(
                    "type definition renamed to '{}', but {} instance update(s) failed: [{}]",
                    to_type.path,
                    failed_instances.len(),
                    failed_instances.join("; ")
                )));
            }
            Ok(json!({
                "rename_type": {
                    "repo": repo,
                    "old_name": old_name,
                    "new_name": new_name,
                    "type_receipt": receipt,
                    "updated_instances": updated_instances,
                }
            }))
        };
        if let Some(req_id) = request_id {
            let args = json!({ "repo": repo, "old_name": old_name, "new_name": new_name });
            let pre = json!({});
            return self.execute_with_outcomes(req_id, "rename_type", repo, &args, &pre, run);
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_inline`: replace a wikilink to `target` inside `locator` with the target note's body.
    pub fn inline_note(
        &self,
        locator: &Locator,
        expected_hash: &str,
        target: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let snap = self.snapshot();
        let caller = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;

        let (target_clean, explicit_repo) = match target.trim().split_once("::") {
            Some((t, r)) => (t.trim(), Some(r.trim())),
            None => (target.trim(), None),
        };
        let target_loc = match snap.resolve_link(&locator.repo, target_clean, explicit_repo) {
            LinkResolution::Resolved(l) => l,
            LinkResolution::Ambiguous(_) => {
                return Err(AppError::InvalidArguments(format!(
                    "ambiguous inline target '{target}'"
                )));
            }
            LinkResolution::Unresolved => {
                return Err(AppError::NotFound(format!(
                    "inline target '{target}' not found"
                )));
            }
        };
        if &target_loc == locator {
            return Err(AppError::InvalidArguments(
                "cannot inline a note into itself".into(),
            ));
        }
        let target_file = snap.files.get(&target_loc).ok_or_else(|| {
            AppError::NotFound(format!("{}/{}", target_loc.repo, target_loc.path))
        })?;

        // Find the first wikilink span in `caller` that resolves to `target_loc`.
        let matching_link = caller.doc.wikilinks.iter().find(|wl| {
            matches!(
                snap.resolve_link(&locator.repo, &wl.target, wl.repo.as_deref()),
                LinkResolution::Resolved(ref l) if l == &target_loc
            )
        });
        let Some(wl) = matching_link else {
            return Err(AppError::NotFound(format!(
                "no wikilink to '{}' found in {}/{}",
                target_loc.path, locator.repo, locator.path
            )));
        };

        let mut updated = String::with_capacity(caller.raw_text.len() + target_file.body().len());
        updated.push_str(&caller.raw_text[..wl.byte_start]);
        updated.push_str(target_file.body().trim());
        updated.push_str(&caller.raw_text[wl.byte_end..]);

        let commit_msg = format!("2brain: inline {} into {}", target_loc.path, locator.path);
        let run = || {
            let receipt =
                self.store
                    .write_file_cas(locator, expected_hash, &updated, &commit_msg)?;
            self.refresh_after_mutation();
            Ok(json!({
                "inline": {
                    "repo": locator.repo,
                    "path": locator.path,
                    "inlined_from": { "repo": target_loc.repo, "path": target_loc.path },
                    "receipt": receipt,
                }
            }))
        };
        if let Some(req_id) = request_id {
            let args = json!({ "repo": locator.repo, "path": locator.path, "target": target });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(req_id, "inline", &locator.repo, &args, &pre, run);
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_promote`: extract a `^block-id` line or heading section from `locator` into a
    /// new note at `new_path`, replacing the source block with a `[[wikilink]]`.
    pub fn promote_block(
        &self,
        locator: &Locator,
        expected_hash: &str,
        source: PromoteSource<'_>,
        new_path: &str,
        new_type: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let (block_id, heading) = match source {
            PromoteSource::BlockId(b) => (Some(b), None),
            PromoteSource::Heading(h) => (None, Some(h)),
        };
        let snap = self.snapshot();
        let source_file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        let new_loc = Locator::new(&locator.repo, new_path)
            .map_err(|e| AppError::InvalidArguments(e.to_string()))?;
        if snap.files.contains_key(&new_loc) {
            return Err(AppError::AlreadyExists(format!(
                "{}/{}",
                new_loc.repo, new_loc.path
            )));
        }

        let (extracted, range_start, range_end) =
            extract_block_or_heading_section(&source_file.raw_text, block_id, heading)?;
        let stem = new_loc
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&new_loc.path)
            .strip_suffix(".md")
            .unwrap_or(&new_loc.path);

        let mut new_content = String::new();
        if let Some(ty) = new_type {
            let ty_clean = validate_slug("new_type", ty)?;
            new_content.push_str(&format!(
                "---\ntype: {ty_clean}\npromoted_from: \"[[{}]]\"\n---\n\n",
                locator.path.strip_suffix(".md").unwrap_or(&locator.path)
            ));
        }
        new_content.push_str(extracted.trim());
        new_content.push('\n');

        let mut updated_source = String::with_capacity(source_file.raw_text.len());
        updated_source.push_str(&source_file.raw_text[..range_start]);
        updated_source.push_str(&format!("[[{stem}]]\n"));
        updated_source.push_str(&source_file.raw_text[range_end..]);

        let create_msg = format!(
            "2brain: promote block from {} to {}",
            locator.path, new_loc.path
        );
        let update_msg = format!(
            "2brain: replace promoted block with [[{stem}]] in {}",
            locator.path
        );
        let run = || {
            // Verify source CAS first so we never create an orphan note if CAS is stale!
            let (_, live_receipt) = self.store.read_exact(locator)?;
            let live_guard = live_receipt.write_guard.to_hex();
            let live_sha = live_receipt.sha256.to_hex();
            let norm = expected_hash
                .strip_prefix("sha256:")
                .unwrap_or(expected_hash);
            if norm != live_guard && !norm.eq_ignore_ascii_case(&live_sha) {
                return Err(AppError::CasMismatch(format!(
                    "expected_hash '{expected_hash}' does not match live '{live_guard}'"
                )));
            }
            let created = self
                .store
                .create_file(&new_loc, &new_content, &create_msg)?;
            let updated = match self.store.write_file_cas(
                locator,
                expected_hash,
                &updated_source,
                &update_msg,
            ) {
                Ok(r) => r,
                Err(source_err) => {
                    let rollback_msg =
                        format!("2brain: rollback failed promote of {}", new_loc.path);
                    match self
                        .store
                        .delete_file_cas(&new_loc, &created.hash, &rollback_msg)
                    {
                        Ok(_) => {
                            self.refresh_after_mutation();
                            return Err(source_err.into());
                        }
                        Err(rb_err) => {
                            self.refresh_after_mutation();
                            return Err(AppError::OutcomeUnknown(format!(
                                "source update failed ({source_err}) after creating '{}/{}', and rollback delete failed ({rb_err})",
                                new_loc.repo, new_loc.path
                            )));
                        }
                    }
                }
            };
            self.refresh_after_mutation();
            Ok(json!({
                "promote": {
                    "source": updated,
                    "created": created,
                }
            }))
        };
        if let Some(req_id) = request_id {
            let args = json!({
                "repo": locator.repo,
                "path": locator.path,
                "block_id": block_id,
                "heading": heading,
                "new_path": new_path,
                "new_type": new_type,
            });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(req_id, "promote", &locator.repo, &args, &pre, run);
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_batch`: execute an ordered sequence of mutations (up to 64 operations),
    /// validating every entry before running.
    pub fn batch_mutations(
        &self,
        operations: &[Value],
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        if operations.is_empty() || operations.len() > 64 {
            return Err(AppError::InvalidArguments(
                "operations must contain 1..=64 items".into(),
            ));
        }
        // Validate every operation shape upfront before mutating anything.
        let mut parsed_ops: Vec<(String, Map<String, Value>)> =
            Vec::with_capacity(operations.len());
        for (idx, item) in operations.iter().enumerate() {
            let Some(obj) = item.as_object() else {
                return Err(AppError::InvalidArguments(format!(
                    "operation[{idx}] must be a JSON object"
                )));
            };
            let Some(op) = obj
                .get("op")
                .or_else(|| obj.get("tool"))
                .and_then(Value::as_str)
            else {
                return Err(AppError::InvalidArguments(format!(
                    "operation[{idx}] missing 'op' field"
                )));
            };
            let verb = op.strip_prefix("tb_").unwrap_or(op);
            if !matches!(
                verb,
                "create_file"
                    | "write_file"
                    | "edit_file"
                    | "delete_file"
                    | "rename"
                    | "assign_block_id"
            ) {
                return Err(AppError::InvalidArguments(format!(
                    "operation[{idx}] unsupported batch op '{op}'"
                )));
            }
            parsed_ops.push((verb.to_owned(), obj.clone()));
        }

        let default_repo = self
            .snapshot()
            .members
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "workspace".to_owned());
        let run = || {
            let mut results = Vec::with_capacity(parsed_ops.len());
            for (idx, (verb, op_args)) in parsed_ops.iter().enumerate() {
                match self.execute_batch_step(verb, op_args) {
                    Ok(res) => results.push(res.payload),
                    Err(e) => {
                        if results.is_empty() {
                            return Err(e);
                        }
                        // Earlier steps have already durably mutated disk: report OutcomeUnknown
                        // (never a pre-effect Rejected state) with exact completed count.
                        return Err(AppError::OutcomeUnknown(format!(
                            "batch partially committed {}/{} step(s) before failing at step {idx} ({verb}): {e}",
                            results.len(),
                            parsed_ops.len()
                        )));
                    }
                }
            }
            Ok(json!({
                "batch": {
                    "completed": results.len(),
                    "total": parsed_ops.len(),
                    "results": results,
                    "generation": self.generation().0,
                }
            }))
        };
        if let Some(req_id) = request_id {
            let args = json!({ "operations": operations });
            let pre = json!({});
            return self.execute_with_outcomes(req_id, "batch", &default_repo, &args, &pre, run);
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    fn execute_batch_step(
        &self,
        verb: &str,
        args: &Map<String, Value>,
    ) -> Result<IdempotentOutcome, AppError> {
        let opt_str = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let req_str = |k: &str| {
            opt_str(k).ok_or_else(|| AppError::InvalidArguments(format!("'{k}' is required")))
        };
        match verb {
            "create_file" => {
                let loc = self.parse_locator_arg(args)?;
                let content = args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'content' is required".into()))?;
                self.create_file(&loc, content, None)
            }
            "write_file" => {
                let loc = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                let content = args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'content' is required".into()))?;
                self.write_file(&loc, expected_hash, content, None)
            }
            "edit_file" => {
                let loc = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                let old_str = args
                    .get("old_str")
                    .or_else(|| args.get("old_string"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'old_str' is required".into()))?;
                let new_str = args
                    .get("new_str")
                    .or_else(|| args.get("new_string"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'new_str' is required".into()))?;
                self.edit_file(&loc, expected_hash, old_str, new_str, None)
            }
            "delete_file" => {
                let loc = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                self.delete_file(&loc, expected_hash, None)
            }
            "rename" => {
                let from = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                let new_path = req_str("new_path").or_else(|_| req_str("to"))?;
                let to = Locator::new(&from.repo, new_path)
                    .map_err(|e| AppError::InvalidArguments(e.to_string()))?;
                self.rename_file(&from, &to, expected_hash, None)
            }
            "assign_block_id" => {
                let loc = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                let block_id = req_str("block_id")?;
                let target = if let Some(ln) = args.get("line").and_then(Value::as_u64) {
                    BlockTarget::Line(ln as usize)
                } else if let Some(m) = opt_str("match_text") {
                    BlockTarget::MatchText(m)
                } else if let Some(h) = opt_str("heading") {
                    BlockTarget::Heading(h)
                } else {
                    BlockTarget::LastLine
                };
                self.assign_block_id(&loc, expected_hash, block_id, target, None)
            }
            other => Err(AppError::Unsupported(format!(
                "unsupported batch op '{other}'"
            ))),
        }
    }

    /// `tb_add_dep` / `tb_remove_dep` (`--allow-composition`).
    pub fn mutate_dep(
        &self,
        repo: &str,
        dep_name_raw: &str,
        add: bool,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let dep_name = validate_slug("dep", dep_name_raw)?;
        let verb = if add { "add_dep" } else { "remove_dep" };
        let run = || {
            let res = self.store.update_repo_deps(repo, &dep_name, add)?;
            self.refresh_after_mutation();
            Ok(json!({ verb: res }))
        };
        if let Some(req_id) = request_id {
            let args = json!({ "repo": repo, "dep": dep_name });
            let pre = json!({});
            return self.execute_with_outcomes(req_id, verb, repo, &args, &pre, run);
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_add_workspace_member` / `tb_remove_workspace_member` / `tb_set_workspace_member_role`.
    pub fn mutate_workspace_member(
        &self,
        action: WorkspaceMemberAction,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let verb = match &action {
            WorkspaceMemberAction::Add { .. } => "add_workspace_member",
            WorkspaceMemberAction::Remove { .. } => "remove_workspace_member",
            WorkspaceMemberAction::SetRole { .. } => "set_workspace_member_role",
        };
        let default_repo = self
            .snapshot()
            .members
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "workspace".to_owned());
        let action_clone = action.clone();
        let run = || {
            let res = self.store.update_workspace_member(action_clone.clone())?;
            self.refresh_after_mutation();
            Ok(json!({ verb: res }))
        };
        if let Some(req_id) = request_id {
            let args = serde_json::to_value(&action).unwrap_or(Value::Null);
            let pre = json!({});
            return self.execute_with_outcomes(req_id, verb, &default_repo, &args, &pre, run);
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }
}

// === Pure Markdown / Frontmatter Mutation Helpers ===

fn insert_block_id_into_content(
    raw: &str,
    block_id: &str,
    line: Option<usize>,
    match_text: Option<&str>,
    heading: Option<&str>,
) -> Result<String, AppError> {
    let trailing_nl = raw.ends_with('\n');
    let mut lines: Vec<String> = raw.lines().map(str::to_owned).collect();
    if lines.is_empty() {
        return Err(AppError::InvalidArguments(
            "cannot assign block_id to empty file".into(),
        ));
    }

    let target_idx = if let Some(ln) = line {
        if ln == 0 || ln > lines.len() {
            return Err(AppError::InvalidArguments(format!(
                "line {ln} out of range (1..={})",
                lines.len()
            )));
        }
        ln - 1
    } else if let Some(needle) = match_text {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .ok_or_else(|| AppError::NotFound(format!("match_text '{needle}' not found")))?
    } else if let Some(h) = heading {
        let h_norm = h.trim().trim_start_matches('#').trim();
        let heading_idx = lines
            .iter()
            .position(|l| {
                let t = l.trim_start();
                t.starts_with('#') && t.trim_start_matches('#').trim() == h_norm
            })
            .ok_or_else(|| AppError::NotFound(format!("heading '{h}' not found")))?;
        // Pick first non-empty body line after the heading (or the heading line itself).
        ((heading_idx + 1)..lines.len())
            .find(|&i| !lines[i].trim().is_empty())
            .unwrap_or(heading_idx)
    } else {
        // Default: last non-empty line in the note.
        lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .ok_or_else(|| AppError::InvalidArguments("no non-empty line in file".into()))?
    };

    let target = &mut lines[target_idx];
    let suffix = format!(" ^{block_id}");
    if target.ends_with(&suffix) {
        return Err(AppError::AlreadyExists(format!(
            "line already carries ^{block_id}"
        )));
    }
    target.push_str(&suffix);

    let mut out = lines.join("\n");
    if trailing_nl {
        out.push('\n');
    }
    Ok(out)
}

fn replace_block_id_definition(
    raw: &str,
    old_marker: &str,
    new_marker: &str,
) -> Result<String, AppError> {
    let trailing_nl = raw.ends_with('\n');
    let mut replaced = false;
    let lines: Vec<String> = raw
        .lines()
        .map(|l| {
            let trimmed_end = l.trim_end();
            if !replaced
                && let Some(prefix) = trimmed_end.strip_suffix(old_marker)
                && (prefix.is_empty() || prefix.ends_with(char::is_whitespace))
            {
                replaced = true;
                return format!("{prefix}{new_marker}");
            }
            l.to_owned()
        })
        .collect();
    if !replaced {
        return Err(AppError::NotFound(format!(
            "block id definition '{old_marker}' not found"
        )));
    }
    let mut out = lines.join("\n");
    if trailing_nl {
        out.push('\n');
    }
    Ok(out)
}

/// Rewrite `^old_id` -> `^new_id` ONLY within the exact parsed `(byte_start, byte_end)`
/// wikilink spans that resolved to the renamed locator.
fn rewrite_block_id_spans(
    raw: &str,
    spans: &[(usize, usize)],
    old_id: &str,
    new_id: &str,
) -> String {
    let mut sorted = spans.to_vec();
    sorted.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
    let needle = format!("^{old_id}");
    let replacement = format!("^{new_id}");
    let mut out = raw.to_owned();
    for (start, end) in sorted {
        if start < end
            && end <= out.len()
            && out.is_char_boundary(start)
            && out.is_char_boundary(end)
        {
            let slice = &out[start..end];
            let rewritten = slice
                .replace(&format!("{needle}]]"), &format!("{replacement}]]"))
                .replace(&format!("{needle}|"), &format!("{replacement}|"))
                .replace(&format!("{needle}:"), &format!("{replacement}:"));
            out.replace_range(start..end, &rewritten);
        }
    }
    out
}

fn replace_frontmatter_field_value(raw: &str, field: &str, old_val: &str, new_val: &str) -> String {
    let Some(rest) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return raw.to_owned();
    };
    let Some(end_idx) = rest.find("\n---") else {
        return raw.to_owned();
    };
    let fm = &rest[..end_idx];
    let tail = &rest[end_idx..];
    let prefix = format!("{field}:");
    let mut changed = false;
    let new_fm: Vec<String> = fm
        .lines()
        .map(|line| {
            if let Some(after) = line.strip_prefix(&prefix) {
                let val = after.trim().trim_matches('"').trim_matches('\'');
                if val == old_val {
                    changed = true;
                    return format!("{field}: {new_val}");
                }
            }
            line.to_owned()
        })
        .collect();
    if !changed {
        return raw.to_owned();
    }
    format!("---\n{}{}", new_fm.join("\n"), tail)
}

fn extract_block_or_heading_section(
    raw: &str,
    block_id: Option<&str>,
    heading: Option<&str>,
) -> Result<(String, usize, usize), AppError> {
    if let Some(bid_raw) = block_id {
        let bid = validate_block_id(bid_raw)?;
        let marker = format!("^{bid}");
        let mut offset = 0usize;
        for line in raw.split_inclusive('\n') {
            let trimmed = line.trim_end_matches(['\r', '\n']).trim_end();
            if let Some(prefix) = trimmed.strip_suffix(&marker)
                && (prefix.is_empty() || prefix.ends_with(char::is_whitespace))
            {
                return Ok((prefix.trim_end().to_owned(), offset, offset + line.len()));
            }
            offset += line.len();
        }
        return Err(AppError::NotFound(format!("block '^{bid}' not found")));
    }

    if let Some(h) = heading {
        let h_norm = h.trim().trim_start_matches('#').trim();
        let mut offset = 0usize;
        let mut start: Option<(usize, usize)> = None; // (byte_start, heading_level)
        for line in raw.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                let level = trimmed.bytes().take_while(|&b| b == b'#').count();
                let title = trimmed[level..].trim();
                if let Some((sec_start, sec_level)) = start {
                    if level <= sec_level {
                        return Ok((raw[sec_start..offset].to_owned(), sec_start, offset));
                    }
                } else if title == h_norm {
                    start = Some((offset, level));
                }
            }
            offset += line.len();
        }
        if let Some((sec_start, _)) = start {
            return Ok((raw[sec_start..raw.len()].to_owned(), sec_start, raw.len()));
        }
        return Err(AppError::NotFound(format!("heading '{h}' not found")));
    }

    Err(AppError::InvalidArguments(
        "promote requires either 'block_id' or 'heading'".into(),
    ))
}

/// Helper used by tests and preview validation.
#[allow(dead_code)]
fn _sha256_hex(s: &str) -> String {
    RevisionDigest::of_bytes(s.as_bytes()).to_hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_id_insert_rename_and_link_rewrite_work_precisely() {
        let src = "# Title\n\nFirst paragraph.\nSecond paragraph.\n";
        let assigned =
            insert_block_id_into_content(src, "key-fact", None, Some("First"), None).unwrap();
        assert!(assigned.contains("First paragraph. ^key-fact\n"));

        let renamed = replace_block_id_definition(&assigned, "^key-fact", "^core-fact").unwrap();
        assert!(renamed.contains("First paragraph. ^core-fact\n"));
        assert!(!renamed.contains("^key-fact"));

        let caller =
            "See [[a#^key-fact|ref]] and [[b#^key-fact]] and text ^key-fact outside links.\n";
        let start = caller.find("[[a#^key-fact|ref]]").unwrap();
        let end = start + "[[a#^key-fact|ref]]".len();
        let rewritten = rewrite_block_id_spans(caller, &[(start, end)], "key-fact", "core-fact");
        assert!(rewritten.contains("[[a#^core-fact|ref]]"));
        assert!(
            rewritten.contains("[[b#^key-fact]]"),
            "unrelated target link with same block id must not be rewritten"
        );
        assert!(rewritten.contains("text ^key-fact outside links."));
    }

    #[test]
    fn extract_section_and_frontmatter_type_rename_preserve_surroundings() {
        let doc = "---\ntype: draft\nstatus: current\n---\n\n# A\nkeep\n\n## Sub\nmove me\n\n# B\nkeep b\n";
        let renamed = replace_frontmatter_field_value(doc, "type", "draft", "note");
        assert!(renamed.contains("type: note\n"));
        assert!(renamed.contains("status: current\n"));

        let (sec, s, e) = extract_block_or_heading_section(doc, None, Some("Sub")).unwrap();
        assert!(sec.contains("## Sub\nmove me"));
        assert!(!sec.contains("# B"));
        assert!(doc[..s].ends_with("# A\nkeep\n\n"));
        assert!(doc[e..].starts_with("# B\nkeep b"));
    }
}
