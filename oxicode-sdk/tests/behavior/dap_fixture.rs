//! DAP debug service fixture (`behavior::dap_service_protocol_scenario`):
//! a scripted python3 fake adapter speaks minimal DAP over stdio and the
//! `DapDebugService` drives the full lifecycle — spawn, initialize, launch,
//! `stopped` observability, breakpoint/stack/variable requests, continue,
//! terminate — through the `BehaviorSessionServices.debug_service` contract.
use crate::common::*;
use oxicode_agent::runtime::{DapDebugService, DebugService};

const FAKE_ADAPTER: &str = r#"
import json, sys

seq = 0

def send(obj):
    global seq
    seq += 1
    obj["seq"] = seq
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

def read_msg():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            raise EOFError
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length":
            length = int(value.strip())
    body = sys.stdin.buffer.read(length)
    return json.loads(body)

def reply(req, body):
    send({
        "type": "response",
        "request_seq": req["seq"],
        "success": True,
        "command": req["command"],
        "body": body,
    })

init = read_msg()
reply(init, {"supportsConfigurationDoneRequest": False})
send({"type": "event", "event": "output", "body": {"output": "adapter up"}})

launch = read_msg()
reply(launch, {})
send({"type": "event", "event": "process", "body": {"name": "fake"}})
send({"type": "event", "event": "stopped",
      "body": {"reason": "entry", "threadId": 1, "allThreadsStopped": True}})

BODIES = {
    "setBreakpoints": {"breakpoints": [{"id": 1, "verified": True, "line": 3}]},
    "threads": {"threads": [{"id": 1, "name": "main"}]},
    "stackTrace": {"stackFrames": [{"id": 100, "name": "main", "line": 3, "column": 1}],
                    "totalFrames": 1},
    "scopes": {"scopes": [{"name": "Locals", "variablesReference": 7}]},
    "variables": {"variables": [{"name": "x", "value": "42", "variablesReference": 0}]},
    "continue": {"allThreadsContinued": True},
}

while True:
    try:
        req = read_msg()
    except EOFError:
        break
    cmd = req.get("command")
    if cmd in ("terminate", "disconnect"):
        reply(req, {})
        send({"type": "event", "event": "exited", "body": {"exitCode": 0}})
        break
    reply(req, BODIES.get(cmd, {}))
"#;

#[tokio::test]
async fn dap_service_protocol_scenario() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();

    let mut services = minimal_services(&ws);
    let service: Arc<dyn DebugService> = Arc::new(DapDebugService::new());
    services.debug_service = Some(service.clone());

    // Manifest: debug-service satisfied, shell/eval still degrade honestly.
    let mut installer = RecordingInstaller::new(WrapMode::Trace);
    let (manifest, _patch) = install_pack_with_services(&services, &mut installer);
    assert!(
        !manifest
            .degraded
            .iter()
            .any(|d| d.feature == "debug-service"),
        "debug-service must be satisfied: {:?}",
        manifest.degraded
    );
    assert!(
        manifest.degraded.iter().any(|d| d.feature == "eval-kernel"),
        "eval-kernel must still degrade"
    );

    // Lifecycle: start (initialize -> launch -> stopped), protocol requests,
    // terminate. The scripted adapter ships inline (`python3 -c`) — no temp
    // file, so the fixture is filesystem-free beyond the workspace root.
    let config = serde_json::json!({
        "type": "fake",
        "request": "launch",
        "adapter": ["python3", "-c", FAKE_ADAPTER],
        "program": "/tmp/fake_program.py",
    });
    let session = service.start(&config).await.expect("start must succeed");

    let bp = service
        .request(
            &session,
            "setBreakpoints",
            &serde_json::json!({"source": {"path": "/tmp/fake_program.py"},
                                 "breakpoints": [{"line": 3}]}),
        )
        .await
        .expect("setBreakpoints");
    assert_eq!(bp["breakpoints"][0]["verified"], serde_json::json!(true));

    let threads = service
        .request(&session, "threads", &serde_json::json!({}))
        .await
        .expect("threads");
    assert_eq!(threads["threads"][0]["name"], serde_json::json!("main"));

    let stack = service
        .request(&session, "stackTrace", &serde_json::json!({"threadId": 1}))
        .await
        .expect("stackTrace");
    assert_eq!(stack["stackFrames"][0]["id"], serde_json::json!(100));

    service
        .request(&session, "scopes", &serde_json::json!({"frameId": 100}))
        .await
        .expect("scopes");
    let vars = service
        .request(
            &session,
            "variables",
            &serde_json::json!({"variablesReference": 7}),
        )
        .await
        .expect("variables");
    assert_eq!(vars["variables"][0]["value"], serde_json::json!("42"));

    let cont = service
        .request(&session, "continue", &serde_json::json!({"threadId": 1}))
        .await
        .expect("continue");
    assert_eq!(cont["allThreadsContinued"], serde_json::json!(true));

    service.terminate(&session).await.expect("terminate");
    // Terminated sessions are gone; further requests fail cleanly.
    assert!(
        service
            .request(&session, "threads", &serde_json::json!({}))
            .await
            .is_err(),
        "requests against a terminated session must fail"
    );

    // Unknown sessions fail cleanly from the start.
    assert!(
        service
            .start(&serde_json::json!({"nope": true}))
            .await
            .is_err()
    );
}
