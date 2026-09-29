//! A Model Context Protocol server, so an agent can drive tidy-up.
//!
//! MCP is JSON-RPC 2.0 over newline-delimited JSON on stdin and stdout, which
//! `serde_json` already covers, so this adds no dependencies.
//!
//! # Why this is safe to expose at all
//!
//! Handing a bulk file mover to an agent is a bad idea unless every action is
//! reviewable before it happens and reversible after. Both were already true:
//! a [`crate::plan::Plan`] is pure data, and every change is journaled. This
//! module adds four more restrictions on top, in rough order of importance:
//!
//! 1. **An agent never supplies paths to move.** It asks for a plan, gets a
//!    `plan_id`, and applies *that*. The moves are the ones tidy-up computed,
//!    not a list the caller composed, so a prompt injection cannot turn
//!    `apply_plan` into "move this file to that one".
//! 2. **Everything is confined to one root**, given on the command line. A
//!    path argument that resolves outside it is refused before anything reads
//!    it.
//! 3. **Read-only by default.** Mutating tools are not even listed without
//!    `--allow-writes`, so an agent cannot discover them and try.
//! 4. **The system-folder guard is not overridable here.** There is no way to
//!    pass `--allow-system-folder` through this interface. If the guard
//!    refuses, that is the answer.
//!
//! # Structure
//!
//! [`Server::handle`] is a pure function from one request to at most one
//! response. The stdio loop around it does nothing but framing, which is what
//! makes the protocol testable without spawning a process or a pipe.

use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{api::PlanFile, error::Result, journal::Operation};

/// Protocol revisions this server understands, newest first.
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC: the method does not exist.
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC: the params were not what the method needs.
const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC: the request was not valid JSON.
const PARSE_ERROR: i64 = -32700;

/// One request off the wire.
#[derive(Debug, Clone)]
pub struct Request {
    /// Absent for a notification, which gets no reply.
    pub id: Option<Value>,
    /// Method name.
    pub method: String,
    /// Parameters, if any.
    pub params: Value,
}

impl Request {
    /// Parses one line. Returns `None` if it is not a JSON-RPC request at all.
    pub fn parse(line: &str) -> Option<Request> {
        let value: Value = serde_json::from_str(line).ok()?;
        Some(Request {
            id: value.get("id").cloned(),
            method: value.get("method")?.as_str()?.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        })
    }

    fn arg<'a>(&'a self, name: &str) -> Option<&'a Value> {
        self.params.get("arguments")?.get(name)
    }

    fn str_arg(&self, name: &str) -> Option<&str> {
        self.arg(name)?.as_str()
    }
}

/// The server, holding the plans it has handed out.
pub struct Server {
    /// Everything is confined to this folder.
    root: PathBuf,
    /// Whether the mutating tools exist at all.
    allow_writes: bool,
    /// Plans this server produced, by handle. An agent may only apply one of
    /// these, never a list of moves of its own.
    plans: BTreeMap<String, PlanFile>,
    /// Counter behind the handles.
    issued: u64,
}

impl Server {
    /// A server confined to `root`.
    pub fn new(root: PathBuf, allow_writes: bool) -> Self {
        Server {
            root,
            allow_writes,
            plans: BTreeMap::new(),
            issued: 0,
        }
    }

    /// Handles one request, returning the response to write back.
    ///
    /// Pure with respect to the protocol: notifications and unknown methods
    /// are decided here, not in the loop. Tool calls do touch the disk.
    pub fn handle(&mut self, request: &Request) -> Option<Value> {
        // A notification has no id and must never be answered.
        let id = request.id.clone()?;
        let result: std::result::Result<Value, String> = match request.method.as_str() {
            "initialize" => Ok(self.initialize(request)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.tools() })),
            "tools/call" => return Some(self.call_tool(&id, request)),
            other => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": METHOD_NOT_FOUND,
                        "message": format!("unknown method: {other}")
                    }
                }));
            }
        };
        Some(match result {
            Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": INVALID_PARAMS, "message": message}
            }),
        })
    }

    fn initialize(&self, request: &Request) -> Value {
        // Echo the client's revision when it is one we know, otherwise offer
        // ours and let it decide, which is what the spec asks for.
        let asked = request
            .params
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let version = if SUPPORTED.contains(&asked) {
            asked
        } else {
            SUPPORTED[0]
        };
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "tidy-up", "version": env!("CARGO_PKG_VERSION")},
            "instructions": self.instructions(),
        })
    }

    /// What the agent is told about how to use this server.
    fn instructions(&self) -> String {
        let mut text = format!(
            "Organizes files under {}. Everything is confined to that folder.\n\n\
             Work in two steps: call a plan_* tool to get a plan and a plan_id, \
             show the plan to the person, then call apply_plan with that id. \
             You cannot specify moves yourself; only a plan tidy-up produced can \
             be applied.\n\n\
             Every change is journaled. show_run explains what a run did and \
             whether it can still be undone; restore_run undoes one.",
            self.root.display()
        );
        if !self.allow_writes {
            text.push_str(
                "\n\nThis server is read-only: it can plan and report, but not \
                 change anything.",
            );
        }
        text
    }

    /// The tools on offer. Mutating ones are absent entirely without
    /// `--allow-writes`, rather than present and failing.
    fn tools(&self) -> Vec<Value> {
        let path_arg = json!({
            "path": {
                "type": "string",
                "description": "Folder, relative to the server root. Omit for the root itself."
            }
        });
        let mut tools = vec![
            json!({
                "name": "analyze",
                "description": "Where the space goes in a folder: totals, biggest files and folders, breakdown by file type, age. Reads metadata only and changes nothing.",
                "inputSchema": {"type": "object", "properties": path_arg},
            }),
            json!({
                "name": "plan_organize",
                "description": "Work out how sorting a folder's loose files into category folders would go. Changes nothing; returns the moves and a plan_id to pass to apply_plan.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_arg["path"],
                        "depth": {"type": "integer", "minimum": 1, "description": "Folder levels to look through. Default 1."}
                    }
                },
            }),
            json!({
                "name": "plan_reorganize",
                "description": "Work out how re-filing a whole tree under new grouping keys would go, unpacking existing folders. Changes nothing; returns the moves and a plan_id.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_arg["path"],
                        "by": {
                            "type": "array",
                            "items": {"type": "string", "enum": ["type", "ext", "year", "month", "day", "size", "alpha"]},
                            "description": "Grouping keys, outermost first, e.g. [\"year\",\"type\"]."
                        }
                    }
                },
            }),
            json!({
                "name": "list_runs",
                "description": "Previous tidy-up runs recorded for a folder, newest last.",
                "inputSchema": {"type": "object", "properties": path_arg},
            }),
            json!({
                "name": "show_run",
                "description": "What one run did, and whether it can still be undone cleanly.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_arg["path"],
                        "run_id": {"type": "string", "description": "Run id. Omit for the most recent."}
                    }
                },
            }),
        ];
        if self.allow_writes {
            tools.push(json!({
                "name": "apply_plan",
                "description": "Carry out a plan produced by a plan_* tool. Takes the plan_id it returned; you cannot supply moves of your own. Returns the journal id, which restore_run undoes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"plan_id": {"type": "string"}},
                    "required": ["plan_id"]
                },
            }));
            tools.push(json!({
                "name": "restore_run",
                "description": "Undo a previous run, putting files back where they were.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_arg["path"],
                        "run_id": {"type": "string", "description": "Run id. Omit for the most recent."}
                    }
                },
            }));
        }
        tools
    }

    fn call_tool(&mut self, id: &Value, request: &Request) -> Value {
        let name = request
            .params
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string();
        match self.run_tool(&name, request) {
            Ok(value) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": value.to_string()}],
                    "structuredContent": value,
                    "isError": false
                }
            }),
            // A tool that refuses is a result, not a protocol failure: the
            // agent needs to read the reason and decide what to do.
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": message}],
                    "isError": true
                }
            }),
        }
    }

    fn run_tool(&mut self, name: &str, request: &Request) -> std::result::Result<Value, String> {
        match name {
            "analyze" => self.analyze(request),
            "plan_organize" => self.plan_organize(request),
            "plan_reorganize" => self.plan_reorganize(request),
            "list_runs" => self.list_runs(request),
            "show_run" => self.show_run(request),
            "apply_plan" if self.allow_writes => self.apply_plan(request),
            "restore_run" if self.allow_writes => self.restore_run(request),
            "apply_plan" | "restore_run" => {
                Err("this server is read-only; it was started without --allow-writes".into())
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }

    // -----------------------------------------------------------------
    // Confinement
    // -----------------------------------------------------------------

    /// Resolves a caller-supplied path inside the server root.
    ///
    /// The check is on the *canonical* path, so a relative path, a `..` or a
    /// symbolic link pointing out of the root are all caught. This is the
    /// boundary the whole server rests on, so it errs toward refusing.
    pub fn resolve(&self, path: Option<&str>) -> std::result::Result<PathBuf, String> {
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            return Ok(self.root.clone());
        };
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.root.join(path)
        };
        let resolved = crate::fsops::resolve_root(&joined)
            .map_err(|e| format!("cannot use {}: {e}", joined.display()))?;
        if !resolved.starts_with(&self.root) {
            return Err(format!(
                "{} is outside {}, which this server is confined to",
                resolved.display(),
                self.root.display()
            ));
        }
        Ok(resolved)
    }

    fn remember(&mut self, plan: PlanFile) -> String {
        self.issued += 1;
        let handle = format!("plan-{}", self.issued);
        self.plans.insert(handle.clone(), plan);
        handle
    }

    // -----------------------------------------------------------------
    // Tools
    // -----------------------------------------------------------------

    fn analyze(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let root = self.resolve(request.str_arg("path"))?;
        let options = crate::analyze::AnalyzeOptions {
            top: 10,
            ..Default::default()
        };
        let report =
            crate::analyze::analyze(&root, &options, &crate::disk::SystemDisks, &mut |_| {})
                .map_err(|e| e.to_string())?;
        serde_json::to_value(report).map_err(|e| e.to_string())
    }

    fn plan_organize(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let root = self.resolve(request.str_arg("path"))?;
        let depth = request
            .arg("depth")
            .and_then(|d| d.as_u64())
            .unwrap_or(1)
            .max(1) as usize;
        let options = crate::scan::ScanOptions {
            max_depth: depth,
            rules: crate::rules::IgnoreRules::new(),
            skip_root_dirs: crate::plan::organize_skip_dirs(),
        };
        let scanned = self.scan(&root, &options)?;
        let plan =
            crate::plan::build_organize_plan(&root, &scanned, crate::plan::ProjectPolicy::Keep);
        Ok(self.describe(plan, Operation::Organize, &scanned))
    }

    fn plan_reorganize(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let root = self.resolve(request.str_arg("path"))?;
        let keys = parse_keys(request.arg("by"))?;
        let options = crate::scan::ScanOptions {
            max_depth: usize::MAX,
            rules: crate::rules::IgnoreRules::new(),
            skip_root_dirs: Default::default(),
        };
        let scanned = self.scan(&root, &options)?;
        let regrouped = crate::regroup::build_reorganize_plan(
            &root,
            &scanned,
            &keys,
            crate::plan::ProjectPolicy::Keep,
            crate::timefmt::utc_offset(),
        );
        Ok(self.describe(regrouped.plan, Operation::Reorganize, &scanned))
    }

    fn scan(
        &self,
        root: &Path,
        options: &crate::scan::ScanOptions,
    ) -> std::result::Result<crate::scan::ScanResult, String> {
        crate::scan::scan(root, options).map_err(|e| e.to_string())
    }

    /// Records a plan and describes it, without the full move list when it is
    /// long: an agent does not need ten thousand lines to decide.
    fn describe(
        &mut self,
        plan: crate::plan::Plan,
        operation: Operation,
        scanned: &crate::scan::ScanResult,
    ) -> Value {
        const SAMPLE: usize = 50;
        let file = PlanFile::of(&plan, operation);
        let (moves, bytes) = (file.moves, file.bytes);
        let sample: Vec<Value> = file
            .items
            .iter()
            .take(SAMPLE)
            .map(|m| {
                json!({
                    "from": rel(&plan.root, &m.from),
                    "to": rel(&plan.root, &m.to),
                    "bytes": m.size
                })
            })
            .collect();
        let mut skipped: BTreeMap<&str, usize> = BTreeMap::new();
        for entry in &scanned.skipped {
            *skipped.entry(entry.reason.key()).or_insert(0) += 1;
        }
        let plan_id = self.remember(file);
        json!({
            "plan_id": plan_id,
            "root": plan.root.display().to_string(),
            "moves": moves,
            "bytes": bytes,
            "sample": sample,
            "sample_truncated": moves > SAMPLE,
            "skipped": skipped,
            "next": "Show this to the person, then call apply_plan with the plan_id if they agree.",
        })
    }

    fn apply_plan(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let handle = request
            .str_arg("plan_id")
            .ok_or("apply_plan needs a plan_id from a plan_* tool")?;
        let file = self
            .plans
            .get(handle)
            .cloned()
            .ok_or_else(|| format!("no plan called {handle}; ask for one first"))?;
        let operation = file.operation;
        let plan = file.into_plan().map_err(|e| e.to_string())?;

        // Confinement is re-checked here, not trusted from when the plan was
        // made, and the system-folder guard applies with no way to override it.
        let root = self.resolve(Some(&plan.root.display().to_string()))?;
        let assessment = crate::safety::assess_for_writing(&root);
        if assessment.risk == crate::safety::Risk::Dangerous {
            return Err(format!(
                "refusing: {} {}",
                assessment.headline(),
                assessment
                    .reasons
                    .first()
                    .map(crate::safety::Reason::explain)
                    .unwrap_or_default()
            ));
        }

        let report =
            crate::executor::execute(&plan, operation, |_| {}).map_err(|e| e.to_string())?;
        self.plans.remove(handle);
        Ok(json!({
            "journal_id": report.journal_id,
            "moved": report.moved,
            "bytes": report.bytes,
            "failed": report.problems.len(),
            "undo": "call restore_run with this journal_id to put everything back",
        }))
    }

    fn list_runs(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let root = self.resolve(request.str_arg("path"))?;
        let runs: Vec<Value> = crate::journal::Journal::load_all(&root)
            .map_err(|e| e.to_string())?
            .iter()
            .map(|j| {
                json!({
                    "run_id": j.header.id,
                    "operation": j.header.operation.to_string(),
                    "when": crate::timefmt::format_utc(j.header.created_at),
                    "moves": j.move_count(),
                    "undone": j.is_restored(),
                })
            })
            .collect();
        Ok(json!({"root": root.display().to_string(), "runs": runs}))
    }

    fn show_run(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let root = self.resolve(request.str_arg("path"))?;
        let journal = self.journal(&root, request)?;
        let review = crate::commands::show::review_of(&root, &journal);
        serde_json::to_value(review).map_err(|e| e.to_string())
    }

    fn restore_run(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let root = self.resolve(request.str_arg("path"))?;
        let mut journal = self.journal(&root, request)?;
        let options = crate::restore::RestoreOptions {
            conflict: crate::restore::ConflictPolicy::Rename,
            dry_run: false,
            // Never over this interface: the server is confined to its root,
            // and a journal is a file an agent may have been pointed at.
            allow_outside: false,
        };
        let report = crate::restore::restore(&root, &mut journal, options, |_, _| {})
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "run_id": journal.header.id,
            "restored": report.restored,
            "renamed": report.renamed.len(),
            "conflicts": report.conflicts.len(),
            "missing": report.missing.len(),
            "failed": report.problems.len(),
        }))
    }

    fn journal(
        &self,
        root: &Path,
        request: &Request,
    ) -> std::result::Result<crate::journal::Journal, String> {
        match request.str_arg("run_id") {
            Some(id) => crate::journal::Journal::find(root, id).map_err(|e| e.to_string()),
            None => crate::journal::Journal::load_all(root)
                .map_err(|e| e.to_string())?
                .pop()
                .ok_or_else(|| format!("no runs recorded for {}", root.display())),
        }
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn parse_keys(value: Option<&Value>) -> std::result::Result<Vec<crate::regroup::GroupBy>, String> {
    use crate::regroup::GroupBy;
    let Some(array) = value.and_then(|v| v.as_array()) else {
        return Ok(vec![GroupBy::Type]);
    };
    array
        .iter()
        .map(|item| match item.as_str().unwrap_or_default() {
            "type" => Ok(GroupBy::Type),
            "ext" => Ok(GroupBy::Ext),
            "year" => Ok(GroupBy::Year),
            "month" => Ok(GroupBy::Month),
            "day" => Ok(GroupBy::Day),
            "size" => Ok(GroupBy::Size),
            "alpha" => Ok(GroupBy::Alpha),
            other => Err(format!(
                "unknown grouping key {other:?}; use type, ext, year, month, day, size or alpha"
            )),
        })
        .collect()
}

/// Reads requests from `input` and writes responses to `output` until the
/// input ends.
///
/// Framing only. A line that is not a request is answered with a parse error
/// when it has an id, and ignored otherwise, because a stream that dies on one
/// bad line is worse than one that carries on.
pub fn serve(server: &mut Server, input: impl BufRead, mut output: impl Write) -> Result<()> {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match Request::parse(&line) {
            Some(request) => server.handle(&request),
            None => Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {"code": PARSE_ERROR, "message": "not a JSON-RPC request"}
            })),
        };
        if let Some(response) = response {
            writeln!(output, "{response}").ok();
            output.flush().ok();
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
