//! Tests for the MCP server.
//!
//! [`Server::handle`] is a pure function from one request to at most one
//! response, so the protocol is exercised directly rather than through a pipe.
//! The safety properties get the most attention, because this is the interface
//! an agent reaches, and an agent will try things a person would not.

use super::*;

fn server(root: &Path, allow_writes: bool) -> Server {
    Server::new(
        crate::fsops::resolve_root(root).expect("a real folder"),
        allow_writes,
    )
}

fn request(id: u64, method: &str, params: Value) -> Request {
    Request {
        id: Some(json!(id)),
        method: method.to_string(),
        params,
    }
}

fn call(server: &mut Server, name: &str, args: Value) -> Value {
    let response = server
        .handle(&request(
            1,
            "tools/call",
            json!({"name": name, "arguments": args}),
        ))
        .expect("a call is answered");
    response["result"].clone()
}

/// The structured half of a successful tool call.
fn ok(server: &mut Server, name: &str, args: Value) -> Value {
    let result = call(server, name, args);
    assert_eq!(result["isError"], false, "expected success: {result}");
    result["structuredContent"].clone()
}

/// The message from a tool that refused.
fn refused(server: &mut Server, name: &str, args: Value) -> String {
    let result = call(server, name, args);
    assert_eq!(result["isError"], true, "expected a refusal: {result}");
    result["content"][0]["text"].as_str().unwrap().to_string()
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), "xx").unwrap();
    std::fs::write(dir.path().join("b.pdf"), "yyy").unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

#[test]
fn initialize_agrees_a_protocol_version() {
    let dir = fixture();
    let mut s = server(dir.path(), false);

    let known = s
        .handle(&request(
            1,
            "initialize",
            json!({"protocolVersion": "2024-11-05"}),
        ))
        .unwrap();
    assert_eq!(known["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(known["result"]["serverInfo"]["name"], "tidy-up");
    assert_eq!(known["jsonrpc"], "2.0");

    // An unknown revision gets ours offered back rather than an error.
    let unknown = s
        .handle(&request(
            2,
            "initialize",
            json!({"protocolVersion": "1999-01-01"}),
        ))
        .unwrap();
    assert_eq!(unknown["result"]["protocolVersion"], SUPPORTED[0]);
}

/// A notification has no id and must never be answered, or the client sees a
/// reply to something it did not ask about.
#[test]
fn a_notification_gets_no_response() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let note = Request {
        id: None,
        method: "notifications/initialized".to_string(),
        params: Value::Null,
    };
    assert!(s.handle(&note).is_none());
}

#[test]
fn an_unknown_method_is_a_protocol_error() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let response = s
        .handle(&request(1, "tools/nonsense", Value::Null))
        .unwrap();
    assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    assert!(response.get("result").is_none());
}

/// A tool refusing is a result the agent must read, not a transport failure.
#[test]
fn a_tool_that_refuses_reports_it_as_a_result_not_an_error() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let response = s
        .handle(&request(
            1,
            "tools/call",
            json!({"name": "no_such_tool", "arguments": {}}),
        ))
        .unwrap();
    assert!(response.get("error").is_none(), "not a protocol error");
    assert_eq!(response["result"]["isError"], true);
}

#[test]
fn a_line_that_is_not_a_request_does_not_stop_the_stream() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let input = concat!(
        "not json at all\n",
        "\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "\n"
    );
    let mut output = Vec::new();
    serve(&mut s, input.as_bytes(), &mut output).unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&output).unwrap().lines().collect();
    assert_eq!(lines.len(), 2, "one parse error, one pong: {lines:?}");
    assert!(lines[0].contains("-32700"));
    assert!(lines[1].contains(r#""id":1"#));
}

// ---------------------------------------------------------------------------
// Read-only by default
// ---------------------------------------------------------------------------

/// The mutating tools are absent, not merely refusing. An agent cannot try
/// what it cannot see.
#[test]
fn a_read_only_server_does_not_offer_the_tools_that_change_things() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let listed = s.handle(&request(1, "tools/list", Value::Null)).unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    for offered in [
        "analyze",
        "plan_organize",
        "plan_reorganize",
        "list_runs",
        "show_run",
    ] {
        assert!(names.contains(&offered), "{offered} missing from {names:?}");
    }
    for withheld in ["apply_plan", "restore_run"] {
        assert!(!names.contains(&withheld), "{withheld} must not be offered");
    }
    assert!(
        listed["result"]["tools"][0]["inputSchema"]["type"] == "object",
        "every tool needs a schema"
    );
}

#[test]
fn calling_a_withheld_tool_says_why_rather_than_pretending_it_is_unknown() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let message = refused(&mut s, "apply_plan", json!({"plan_id": "plan-1"}));
    assert!(message.contains("read-only"), "{message}");
    assert!(message.contains("--allow-writes"), "{message}");
}

#[test]
fn a_writable_server_offers_them() {
    let dir = fixture();
    let mut s = server(dir.path(), true);
    let listed = s.handle(&request(1, "tools/list", Value::Null)).unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"apply_plan"));
    assert!(names.contains(&"restore_run"));
}

// ---------------------------------------------------------------------------
// Confinement
// ---------------------------------------------------------------------------

/// The boundary the whole server rests on. An agent acting on something it
/// read must not be able to reach out of the folder it was given.
#[test]
fn a_path_outside_the_root_is_refused() {
    let outer = tempfile::tempdir().unwrap();
    let inner = outer.path().join("served");
    std::fs::create_dir(&inner).unwrap();
    std::fs::write(outer.path().join("secret.txt"), "not yours").unwrap();
    let mut s = server(&inner, true);

    for escape in ["..", "../", "../secret.txt"] {
        let message = refused(&mut s, "analyze", json!({"path": escape}));
        assert!(
            message.contains("outside") || message.contains("cannot use"),
            "{escape} was not refused: {message}"
        );
    }

    let absolute = outer.path().display().to_string();
    let message = refused(&mut s, "analyze", json!({"path": absolute}));
    assert!(message.contains("outside"), "{message}");
}

/// Resolution is on the canonical path, so a link pointing out of the root is
/// caught even though the name looks innocent.
#[cfg(unix)]
#[test]
fn a_symlink_pointing_out_of_the_root_is_refused() {
    let outer = tempfile::tempdir().unwrap();
    let inner = outer.path().join("served");
    std::fs::create_dir(&inner).unwrap();
    let elsewhere = outer.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, inner.join("door")).unwrap();

    let mut s = server(&inner, true);
    let message = refused(&mut s, "analyze", json!({"path": "door"}));
    assert!(message.contains("outside"), "{message}");
}

#[test]
fn no_path_means_the_root_itself() {
    let dir = fixture();
    let s = server(dir.path(), false);
    assert_eq!(
        s.resolve(None).unwrap(),
        crate::fsops::resolve_root(dir.path()).unwrap()
    );
    assert_eq!(s.resolve(Some("")).unwrap(), s.resolve(None).unwrap());
}

// ---------------------------------------------------------------------------
// Propose then apply
// ---------------------------------------------------------------------------

#[test]
fn a_plan_is_proposed_then_applied_by_handle() {
    let dir = fixture();
    let mut s = server(dir.path(), true);

    let proposed = ok(&mut s, "plan_organize", json!({}));
    assert_eq!(proposed["moves"], 2, "{proposed}");
    let plan_id = proposed["plan_id"].as_str().unwrap().to_string();
    assert!(proposed["sample"].as_array().unwrap().len() == 2);
    // Planning changes nothing.
    assert!(dir.path().join("a.png").exists());

    let applied = ok(&mut s, "apply_plan", json!({"plan_id": plan_id}));
    assert_eq!(applied["moved"], 2, "{applied}");
    assert!(dir.path().join("Images").join("a.png").exists());

    let journal = applied["journal_id"].as_str().unwrap().to_string();
    let undone = ok(&mut s, "restore_run", json!({"run_id": journal}));
    assert_eq!(undone["restored"], 2, "{undone}");
    assert!(dir.path().join("a.png").exists(), "put back");
}

/// The property that matters most here: the agent never says what to move. It
/// asks for a plan and applies that plan, so a prompt injection cannot turn
/// apply_plan into "move this file over that one".
#[test]
fn an_agent_cannot_supply_moves_of_its_own() {
    let dir = fixture();
    let mut s = server(dir.path(), true);

    // There is no argument for it: the schema takes a plan_id and nothing else.
    let listed = s.handle(&request(1, "tools/list", Value::Null)).unwrap();
    let apply = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "apply_plan")
        .unwrap()
        .clone();
    let properties = apply["inputSchema"]["properties"].as_object().unwrap();
    assert_eq!(properties.len(), 1, "only a plan_id: {properties:?}");
    assert!(properties.contains_key("plan_id"));

    // And an invented handle is not honoured.
    let message = refused(&mut s, "apply_plan", json!({"plan_id": "plan-999"}));
    assert!(message.contains("no plan called"), "{message}");
}

/// A plan is consumed when applied, so a replay cannot run the same moves a
/// second time against a folder that has already changed.
#[test]
fn a_plan_cannot_be_applied_twice() {
    let dir = fixture();
    let mut s = server(dir.path(), true);
    let plan_id = ok(&mut s, "plan_organize", json!({}))["plan_id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(&mut s, "apply_plan", json!({"plan_id": plan_id.clone()}));
    let message = refused(&mut s, "apply_plan", json!({"plan_id": plan_id}));
    assert!(message.contains("no plan called"), "{message}");
}

#[test]
fn plan_reorganize_takes_grouping_keys_and_rejects_nonsense() {
    let dir = fixture();
    let mut s = server(dir.path(), true);

    let proposed = ok(&mut s, "plan_reorganize", json!({"by": ["type", "ext"]}));
    assert_eq!(proposed["moves"], 2, "{proposed}");
    let to = proposed["sample"][0]["to"].as_str().unwrap();
    assert!(to.contains("Images"), "{to}");

    let message = refused(&mut s, "plan_reorganize", json!({"by": ["colour"]}));
    assert!(message.contains("colour"), "{message}");
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

#[test]
fn the_reporting_tools_answer_without_changing_anything() {
    let dir = fixture();
    let mut s = server(dir.path(), true);

    let analysis = ok(&mut s, "analyze", json!({}));
    assert_eq!(analysis["files"], 2, "{analysis}");

    let empty = ok(&mut s, "list_runs", json!({}));
    assert_eq!(empty["runs"].as_array().unwrap().len(), 0);

    let plan_id = ok(&mut s, "plan_organize", json!({}))["plan_id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(&mut s, "apply_plan", json!({"plan_id": plan_id}));

    let runs = ok(&mut s, "list_runs", json!({}));
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1, "{runs}");

    let review = ok(&mut s, "show_run", json!({}));
    assert_eq!(review["moves"], 2, "{review}");
    assert_eq!(review["in_place"], 2);
    assert_eq!(review["restored"], false);
}

#[test]
fn asking_about_a_folder_with_no_runs_says_so() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let message = refused(&mut s, "show_run", json!({}));
    assert!(message.contains("no runs recorded"), "{message}");
}

/// An agent should not have to read ten thousand moves to decide, and a
/// response that large helps nobody.
#[test]
fn a_large_plan_is_sampled_and_says_that_it_was() {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..60 {
        std::fs::write(dir.path().join(format!("file{n}.png")), "x").unwrap();
    }
    let mut s = server(dir.path(), false);
    let proposed = ok(&mut s, "plan_organize", json!({}));
    assert_eq!(proposed["moves"], 60);
    assert_eq!(proposed["sample"].as_array().unwrap().len(), 50);
    assert_eq!(proposed["sample_truncated"], true);
}

/// The instructions are what an agent reads before doing anything, so they
/// have to state the two-step shape and the confinement.
#[test]
fn the_instructions_explain_how_to_use_the_server() {
    let dir = fixture();
    let mut s = server(dir.path(), false);
    let text = s.handle(&request(1, "initialize", json!({}))).unwrap()["result"]["instructions"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("plan_id"), "{text}");
    assert!(text.contains("confined"), "{text}");
    assert!(
        text.contains("read-only"),
        "a read-only server says so: {text}"
    );
}
