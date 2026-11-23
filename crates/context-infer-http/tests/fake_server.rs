//! Talks to a one-shot HTTP server on localhost; no network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use context_infer::{
    Device, EmbedPurpose, GenRequest, Image, InferError, Job, LoadSpec, Media, Message, Output,
    PixelFormat, Priority, Registry, Runtime, RuntimeBackend, Sampling, Task,
};
use context_infer_http::{Authentication, CredentialSource, Endpoint, HttpBackend};

const REGISTRY: &str = r#"(
    version: 2,
    models: {
        "chat": (capabilities: [Generate, Vision, JsonOutput], licence: "Apache-2.0",
                 artifacts: [Remote(name: "served-chat")]),
        "vectors": (capabilities: [Embed], licence: "MIT", embedding_dimensions: Some(2),
                    query_prefix: "query: ", artifacts: [Remote(name: "served-vectors")]),
    },
    tasks: {
        SummarizeSegments: [(model: "chat", runtime: OpenAiCompatible, device: Remote)],
        EmbedText: [(model: "vectors", runtime: OpenAiCompatible, device: Remote)],
    },
)"#;

#[derive(Debug)]
struct FixedKey;

impl CredentialSource for FixedKey {
    fn authorization(&self) -> Result<Option<String>, InferError> {
        Ok(Some("Bearer test-key".into()))
    }
}

/// Answers one request with `status` and `body`, and gives back what it received.
fn serve_once(status: &'static str, body: &'static str) -> (String, JoinHandle<String>) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(length) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = length.trim().parse().unwrap();
            }
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        reader.get_mut().write_all(response.as_bytes()).unwrap();
        head + &String::from_utf8(body).unwrap()
    });
    (base_url, server)
}

fn backend(base_url: String) -> HttpBackend {
    let endpoint = Endpoint {
        base_url,
        authentication: Authentication::None,
    };
    HttpBackend::with_credentials(&endpoint, Arc::new(FixedKey)).unwrap()
}

fn with_spec<T>(task: Task, use_spec: impl FnOnce(&LoadSpec<'_>) -> T) -> T {
    let registry = Registry::from_ron(REGISTRY).unwrap();
    let candidate = registry.candidates(task)[0].clone();
    assert_eq!(
        (candidate.runtime, candidate.device),
        (Runtime::OpenAiCompatible, Device::Remote)
    );
    use_spec(&LoadSpec::new(&registry, &candidate, Path::new("/unused")).unwrap())
}

fn request(media: Vec<Media>) -> GenRequest {
    GenRequest {
        messages: vec![
            Message::system("Answer in JSON."),
            Message::user("Describe."),
        ],
        media,
        output: Output::json_from_text(r#"{"type": "object"}"#).unwrap(),
        sampling: Sampling::default(),
        max_tokens: 64,
        job: Job::within(Duration::from_secs(10), Priority::Interactive),
    }
}

#[test]
fn sends_the_schema_the_key_and_a_png_and_reads_the_answer() {
    let (base_url, server) = serve_once(
        "200 OK",
        r#"{"choices":[{"message":{"content":"{\"ok\":true}"}}],"usage":null}"#,
    );
    let pixel = Image::new(1, 1, 4, PixelFormat::Xrgb8888, vec![0, 0, 255, 0].into()).unwrap();
    let generation = with_spec(Task::SummarizeSegments, |spec| {
        backend(base_url)
            .load_generator(spec)
            .unwrap()
            .generate(&request(vec![Media::Image(pixel)]))
    })
    .unwrap();
    assert_eq!(generation.text, "{\"ok\":true}");
    let received = server.join().unwrap();
    assert!(received.starts_with("POST /v1/chat/completions"));
    assert!(
        received
            .to_ascii_lowercase()
            .contains("authorization: bearer test-key")
    );
    assert!(received.contains("\"model\":\"served-chat\""), "{received}");
    assert!(received.contains("\"json_schema\""));
    assert!(received.contains("data:image/png;base64,"));
}

#[test]
fn too_many_requests_is_busy_with_the_server_retry_time() {
    let (base_url, server) = serve_once("429 Too Many Requests\r\nRetry-After: 7", "");
    let result = with_spec(Task::SummarizeSegments, |spec| {
        backend(base_url)
            .load_generator(spec)
            .unwrap()
            .generate(&request(Vec::new()))
    });
    server.join().unwrap();
    assert!(matches!(
        result,
        Err(InferError::Busy { retry_after }) if retry_after == Duration::from_secs(7)
    ));
}

#[test]
fn embeddings_come_back_in_input_order_with_unit_length() {
    let (base_url, server) = serve_once(
        "200 OK",
        r#"{"data":[{"index":1,"embedding":[0,2,9]},{"index":0,"embedding":[3,4,9]}]}"#,
    );
    let embeddings = with_spec(Task::EmbedText, |spec| {
        backend(base_url)
            .load_embedder(spec)
            .unwrap()
            .embed(&["a".into(), "b".into()], EmbedPurpose::Query)
    })
    .unwrap();
    assert_eq!(embeddings.values, [0.6, 0.8, 0.0, 1.0]);
    assert!(server.join().unwrap().contains("\"query: a\""));
}
