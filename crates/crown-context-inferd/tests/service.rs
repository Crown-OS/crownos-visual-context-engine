//! Starts the daemon on a private socket and talks to it as a client. The first test needs no
//! model; the ignored one needs `$CROWN_CONTEXT_MODELS`, and `$CROWN_CONTEXT_MODELS_OVERRIDE`
//! may name a user registry file to put on top.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use context_protocol::inference::Client;
use context_protocol::{
    INFERENCE_SERVICE, InferFailure, JobPriority, JobTicket, SharedImage, WirePixelFormat,
};
use context_shm::share_read_only;

const REPLY_TIMEOUT: Option<Duration> = Some(Duration::from_secs(30));

/// Kills the daemon it started, by its own handle, and removes its directory when the test
/// ends.
struct Daemon {
    child: Child,
    directory: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn start(directory: &Path, models: &Path) -> Daemon {
    let registry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/registry.ron");
    let child = Command::new(env!("CARGO_BIN_EXE_crown-context-inferd"))
        .arg("--registry")
        .arg(registry)
        .arg("--socket-dir")
        .arg(directory)
        .env("CROWN_CONTEXT_MODELS", models)
        .env("XDG_CONFIG_HOME", directory.join("config"))
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .env("ORT_DYLIB_PATH", directory.join("no-onnxruntime.so"))
        .spawn()
        .unwrap();
    let daemon = Daemon {
        child,
        directory: directory.to_owned(),
    };
    let socket = directory.join(format!("{INFERENCE_SERVICE}.sock"));
    let started = Instant::now();
    while !socket.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the daemon did not start"
        );
        thread::sleep(Duration::from_millis(50));
    }
    daemon
}

const fn ticket() -> JobTicket {
    JobTicket {
        job_id: 1,
        priority: JobPriority::Interactive,
        budget_ms: 5_000,
    }
}

#[test]
fn answers_capabilities_and_refuses_what_it_cannot_run() {
    let directory = std::env::temp_dir().join(format!("inferd-service-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let _daemon = start(&directory, &directory.join("models"));
    let mut client = Client::connect_in(&directory).unwrap();

    let pending = client.capabilities().unwrap();
    let capabilities = client.wait(pending, REPLY_TIMEOUT).unwrap();
    assert!(
        capabilities
            .templates
            .iter()
            .any(|id| id == "summarize_segments")
    );
    assert!(
        capabilities
            .routes
            .iter()
            .flat_map(|route| &route.candidates)
            .all(|candidate| !candidate.contains("Remote"))
    );

    let frame = share_read_only(c"frame", &[255; 64 * 32 * 4]).unwrap();
    let layout = SharedImage {
        width: 64,
        height: 32,
        stride: 64 * 4,
        format: WirePixelFormat::Xrgb8888,
    };
    let pending = client.ocr(ticket(), layout, frame).unwrap();
    assert!(matches!(
        client.wait(pending, REPLY_TIMEOUT).unwrap(),
        Err(InferFailure::Unavailable(_))
    ));

    let pending = client
        .generate(ticket(), "no_such_template".into(), Vec::new(), None, 16)
        .unwrap();
    assert!(matches!(
        client.wait(pending, REPLY_TIMEOUT).unwrap(),
        Err(InferFailure::Unavailable(_))
    ));

    let unsealed = std::fs::File::open("/dev/null").unwrap();
    let pending = client.ocr(ticket(), layout, unsealed.into()).unwrap();
    assert!(matches!(
        client.wait(pending, REPLY_TIMEOUT).unwrap(),
        Err(InferFailure::InvalidInput(_))
    ));
}

#[test]
#[ignore = "needs a summarize_segments model in $CROWN_CONTEXT_MODELS"]
fn generates_schema_json_through_the_socket() {
    let directory = std::env::temp_dir().join(format!("inferd-generate-{}", std::process::id()));
    let user_config = directory.join("config").join("crownos");
    std::fs::create_dir_all(&user_config).unwrap();
    if let Some(user_override) = std::env::var_os("CROWN_CONTEXT_MODELS_OVERRIDE") {
        std::fs::copy(user_override, user_config.join("models.ron")).unwrap();
    }
    let models = PathBuf::from(std::env::var_os("CROWN_CONTEXT_MODELS").unwrap());
    let _daemon = start(&directory, &models);
    let mut client = Client::connect_in(&directory).unwrap();
    let schema = r#"{"type": "object", "properties": {"segments": {"type": "array", "items": {"type": "object", "properties": {"summary": {"type": "string"}}, "required": ["summary"]}}}, "required": ["segments"]}"#;
    let vars = vec![(
        "segments".to_owned(),
        "## Segment 1\nApp: Zed\nTitle: router.rs\nText:\nfn ranked(&self)".to_owned(),
    )];
    let mut long_ticket = ticket();
    long_ticket.budget_ms = 300_000;
    let pending = client
        .generate(
            long_ticket,
            "summarize_segments".into(),
            vars,
            Some(schema.into()),
            200,
        )
        .unwrap();
    let text = client
        .wait(pending, Some(Duration::from_secs(300)))
        .unwrap()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(value["segments"].is_array());
}
