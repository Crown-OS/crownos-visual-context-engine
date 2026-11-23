#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crownos_config::system::ai::{Device, Runtime};

use super::{BenchmarkCache, LoadSpec, Policy, Router, RouterSettings, RuntimeBackend};
use crate::capability::{
    Embed, EmbedPurpose, EmbedSpace, Embeddings, Generate, GenerateCapabilities,
};
use crate::error::InferError;
use crate::registry::{Capability, Registry, Tier};
use crate::request::{GenRequest, Generation, Job, Output, Priority, Sampling};
use crate::task::Task;

const REGISTRY: &str = r#"(
    version: 2,
    models: {
        "big": (capabilities: [Generate, Vision, JsonOutput], licence: "Apache-2.0",
                artifacts: [Gguf(model: (path: "big.gguf"), quant: "Q4_K_M"), Remote(name: "big")]),
        "small": (capabilities: [Generate, Vision, JsonOutput], licence: "Apache-2.0",
                  artifacts: [Gguf(model: (path: "small.gguf"), quant: "Q4_K_M")]),
        "embed-a": (capabilities: [Embed], licence: "Apache-2.0", embedding_dimensions: Some(4),
                    artifacts: [Onnx(graphs: {"encoder": (path: "a.onnx")}),
                                Gguf(model: (path: "a.gguf"), quant: "Q8_0")]),
        "embed-b": (capabilities: [Embed], licence: "MIT", embedding_dimensions: Some(8),
                    artifacts: [Onnx(graphs: {"encoder": (path: "b.onnx")})]),
    },
    tasks: {
        DescribeScreen: [
            (model: "big", runtime: LlamaCpp, device: Cuda, min_tier: High),
            (model: "small", runtime: LlamaCpp, device: Cuda),
            (model: "small", runtime: LlamaCpp, device: Vulkan),
            (model: "small", runtime: LlamaCpp, device: Cpu, on_battery: false),
            (model: "big", runtime: OpenAiCompatible, device: Remote),
        ],
        EmbedText: [
            (model: "embed-a", runtime: OnnxRuntime, device: Cuda),
            (model: "embed-a", runtime: LlamaCpp, device: Vulkan),
            (model: "embed-b", runtime: OnnxRuntime, device: Cpu),
        ],
    },
)"#;

type Behaviours = Arc<Mutex<HashMap<(String, Device), Behaviour>>>;

#[derive(Clone, Copy, Debug)]
enum Behaviour {
    Answer,
    Fail,
    Busy,
    OwnTimeout,
    Hang,
}

/// One fake per runtime. Calls are logged as `model@Device`.
#[derive(Debug)]
struct FakeRuntime {
    runtime: Runtime,
    devices: Vec<Device>,
    behaviours: Behaviours,
    log: Arc<Mutex<Vec<String>>>,
    loads: Arc<Mutex<usize>>,
    load_time: Mutex<Duration>,
}

impl FakeRuntime {
    fn new(runtime: Runtime, devices: &[Device], log: &Arc<Mutex<Vec<String>>>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            devices: devices.to_vec(),
            behaviours: Behaviours::default(),
            log: Arc::clone(log),
            loads: Arc::new(Mutex::new(0)),
            load_time: Mutex::new(Duration::ZERO),
        })
    }

    fn set(&self, model: &str, device: Device, behaviour: Behaviour) {
        self.behaviours
            .lock()
            .unwrap()
            .insert((model.to_owned(), device), behaviour);
    }

    fn fake_model(&self, spec: &LoadSpec<'_>) -> FakeModel {
        thread::sleep(*self.load_time.lock().unwrap());
        *self.loads.lock().unwrap() += 1;
        FakeModel {
            label: format!("{}@{:?}", spec.model_id, spec.device),
            key: (spec.model_id.to_owned(), spec.device),
            behaviours: Arc::clone(&self.behaviours),
            space: spec.embed_space(),
            log: Arc::clone(&self.log),
        }
    }
}

impl RuntimeBackend for FakeRuntime {
    fn runtime(&self) -> Runtime {
        self.runtime
    }

    fn version(&self) -> String {
        "fake-1".into()
    }

    fn probe(&self, device: Device) -> bool {
        self.devices.contains(&device)
    }

    fn load_generator(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Generate>, InferError> {
        Ok(Arc::new(self.fake_model(spec)))
    }

    fn load_embedder(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Embed>, InferError> {
        Ok(Arc::new(self.fake_model(spec)))
    }
}

#[derive(Debug)]
struct FakeModel {
    label: String,
    key: (String, Device),
    behaviours: Behaviours,
    space: Option<EmbedSpace>,
    log: Arc<Mutex<Vec<String>>>,
}

impl FakeModel {
    fn behave(&self) -> Result<(), InferError> {
        self.log.lock().unwrap().push(self.label.clone());
        let behaviour = self.behaviours.lock().unwrap().get(&self.key).copied();
        match behaviour.unwrap_or(Behaviour::Answer) {
            Behaviour::Answer => Ok(()),
            Behaviour::Fail => Err(InferError::Backend("fake failure".into())),
            Behaviour::Busy => Err(InferError::Busy {
                retry_after: Duration::from_millis(5),
            }),
            Behaviour::OwnTimeout => Err(InferError::Timeout),
            Behaviour::Hang => {
                thread::sleep(Duration::from_secs(5));
                Ok(())
            }
        }
    }
}

impl Generate for FakeModel {
    fn capabilities(&self) -> GenerateCapabilities {
        GenerateCapabilities {
            images: true,
            json_schema: true,
            context_tokens: 4096,
        }
    }

    fn stream(
        &self,
        _request: &GenRequest,
        on_text: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<Generation, InferError> {
        self.behave()?;
        let _ = on_text(&self.label);
        Ok(Generation {
            text: self.label.clone(),
            ..Generation::default()
        })
    }
}

impl Embed for FakeModel {
    fn space(&self) -> &EmbedSpace {
        self.space.as_ref().unwrap()
    }

    fn embed(&self, texts: &[String], _purpose: EmbedPurpose) -> Result<Embeddings, InferError> {
        self.behave()?;
        let dimensions = self.space().dimensions as usize;
        Ok(Embeddings {
            dimensions,
            values: vec![0.5; dimensions * texts.len()],
        })
    }
}

struct Harness {
    router: Router,
    llama: Arc<FakeRuntime>,
    onnx: Arc<FakeRuntime>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    fn new(llama_devices: &[Device], onnx_devices: &[Device], settings: RouterSettings) -> Self {
        let log = Arc::new(Mutex::new(Vec::new()));
        let llama = FakeRuntime::new(Runtime::LlamaCpp, llama_devices, &log);
        let onnx = FakeRuntime::new(Runtime::OnnxRuntime, onnx_devices, &log);
        let remote = FakeRuntime::new(Runtime::OpenAiCompatible, &[Device::Remote], &log);
        let backends: Vec<Arc<dyn RuntimeBackend>> = vec![llama.clone(), onnx.clone(), remote];
        let router = Router::new(Registry::from_ron(REGISTRY).unwrap(), backends, settings);
        Self {
            router,
            llama,
            onnx,
            log,
        }
    }

    fn all_devices() -> Self {
        Self::new(
            &[Device::Cuda, Device::Vulkan, Device::Cpu],
            &[Device::Cuda, Device::Cpu],
            settings(),
        )
    }

    fn describe(&self, budget: Duration) -> Result<String, InferError> {
        let request = GenRequest {
            messages: Vec::new(),
            media: Vec::new(),
            output: Output::Text,
            sampling: Sampling::default(),
            max_tokens: 16,
            job: Job::within(budget, Priority::Interactive),
        };
        self.router
            .generate(Task::DescribeScreen, request)
            .map(|generation| generation.text)
    }

    fn embed(&self, budget: Duration) -> Result<EmbedSpace, InferError> {
        self.router
            .embed(
                vec!["a".into(), "b".into()],
                EmbedPurpose::Document,
                Job::within(budget, Priority::Background),
            )
            .map(|embedded| embedded.space)
    }

    fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn settings() -> RouterSettings {
    let mut settings = RouterSettings::new("/nonexistent/models".into());
    settings.watchdog_grace = Duration::from_millis(20);
    settings
}

const SECOND: Duration = Duration::from_secs(1);

#[test]
fn takes_the_best_candidate_that_the_probe_and_policy_allow() {
    let harness = Harness::new(&[Device::Vulkan, Device::Cpu], &[], settings());
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");

    let mut high_tier = settings();
    high_tier.policy.tier = Tier::High;
    let harness = Harness::new(&[Device::Cuda], &[], high_tier);
    assert_eq!(harness.describe(SECOND).unwrap(), "big@Cuda");
}

#[test]
fn on_battery_skips_candidates_that_need_mains_power() {
    let harness = Harness::new(&[Device::Cpu], &[], settings());
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Cpu");
    harness.router.set_policy(Policy {
        on_battery: true,
        ..Policy::default()
    });
    assert!(matches!(
        harness.describe(SECOND),
        Err(InferError::Unavailable(_))
    ));
}

#[test]
fn remote_needs_consent() {
    let harness = Harness::new(&[], &[], settings());
    assert!(harness.describe(SECOND).is_err());
    harness.router.set_policy(Policy {
        remote_consent: true,
        ..Policy::default()
    });
    assert_eq!(harness.describe(SECOND).unwrap(), "big@Remote");
}

#[test]
fn an_error_falls_through_and_opens_the_breaker() {
    let harness = Harness::all_devices();
    harness.llama.set("small", Device::Cuda, Behaviour::Fail);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(
        harness.calls(),
        ["small@Cuda", "small@Vulkan", "small@Vulkan"]
    );
}

#[test]
fn busy_falls_through_without_opening_the_breaker() {
    let harness = Harness::all_devices();
    harness.llama.set("small", Device::Cuda, Behaviour::Busy);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(
        harness.calls(),
        ["small@Cuda", "small@Vulkan", "small@Cuda", "small@Vulkan"]
    );
}

#[test]
fn a_hung_call_times_out_and_the_next_call_avoids_it() {
    let harness = Harness::all_devices();
    harness.llama.set("small", Device::Cuda, Behaviour::Hang);
    let started = Instant::now();
    assert!(matches!(
        harness.describe(Duration::from_millis(100)),
        Err(InferError::Timeout)
    ));
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
}

#[test]
fn a_hung_fixed_cost_call_with_a_known_latency_falls_through_before_the_deadline() {
    let harness = Harness::all_devices();
    harness.embed(SECOND).unwrap();
    harness.onnx.set("embed-a", Device::Cuda, Behaviour::Hang);
    let started = Instant::now();
    harness.embed(3 * SECOND).unwrap();
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert_eq!(
        harness.calls(),
        ["embed-a@Cuda", "embed-a@Cuda", "embed-a@Vulkan"]
    );
}

#[test]
fn a_load_slower_than_the_deadline_keeps_the_model_and_the_breaker_closed() {
    let harness = Harness::all_devices();
    *harness.llama.load_time.lock().unwrap() = Duration::from_millis(300);
    assert!(matches!(
        harness.describe(Duration::from_millis(50)),
        Err(InferError::Timeout)
    ));
    thread::sleep(Duration::from_millis(400));
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Cuda");
    assert_eq!(*harness.llama.loads.lock().unwrap(), 1);
}

#[test]
fn a_backend_that_stops_at_its_deadline_keeps_the_breaker_closed() {
    let harness = Harness::all_devices();
    harness
        .llama
        .set("small", Device::Cuda, Behaviour::OwnTimeout);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(
        harness.calls(),
        ["small@Cuda", "small@Vulkan", "small@Cuda", "small@Vulkan"]
    );
}

#[test]
fn open_breakers_answer_busy_with_a_retry_time() {
    let harness = Harness::new(&[Device::Cuda], &[], settings());
    harness.llama.set("small", Device::Cuda, Behaviour::Fail);
    assert!(matches!(
        harness.describe(SECOND),
        Err(InferError::Backend(_))
    ));
    assert!(matches!(
        harness.describe(SECOND),
        Err(InferError::Busy { retry_after }) if retry_after > Duration::from_secs(20)
    ));
}

#[test]
fn measured_latency_orders_the_devices_of_one_model() {
    let candidates = Registry::from_ron(REGISTRY).unwrap().tasks[&Task::DescribeScreen].clone();
    let mut benchmarks = BenchmarkCache::default();
    benchmarks.record(
        Task::DescribeScreen,
        &candidates[1],
        Duration::from_millis(50),
    );
    benchmarks.record(
        Task::DescribeScreen,
        &candidates[2],
        Duration::from_millis(30),
    );
    benchmarks.record(
        Task::DescribeScreen,
        &candidates[3],
        Duration::from_millis(10),
    );
    let mut measured = settings();
    measured.benchmarks = benchmarks;
    let harness = Harness::new(&[Device::Cuda, Device::Vulkan, Device::Cpu], &[], measured);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Cpu");
}

#[test]
fn a_candidate_too_slow_for_the_deadline_is_skipped() {
    let candidates = Registry::from_ron(REGISTRY).unwrap().tasks[&Task::EmbedText].clone();
    let mut benchmarks = BenchmarkCache::default();
    benchmarks.record(Task::EmbedText, &candidates[0], Duration::from_secs(5));
    let mut measured = settings();
    measured.benchmarks = benchmarks;
    let harness = Harness::new(&[Device::Vulkan], &[Device::Cuda], measured);
    harness.embed(SECOND).unwrap();
    assert_eq!(harness.calls(), ["embed-a@Vulkan"]);
}

#[test]
fn embedding_never_falls_through_to_another_space() {
    let harness = Harness::all_devices();
    harness.onnx.set("embed-a", Device::Cuda, Behaviour::Fail);
    harness
        .llama
        .set("embed-a", Device::Vulkan, Behaviour::Fail);
    assert_eq!(
        harness.router.embed_space().unwrap().to_string(),
        "embed-a/4"
    );
    assert!(harness.embed(SECOND).is_err());
    assert_eq!(harness.calls(), ["embed-a@Cuda", "embed-a@Vulkan"]);
}

#[test]
fn the_space_changes_only_when_its_model_cannot_run() {
    let harness = Harness::new(&[], &[Device::Cpu], settings());
    assert_eq!(
        harness.router.embed_space().unwrap().to_string(),
        "embed-b/8"
    );
    assert_eq!(harness.embed(SECOND).unwrap().to_string(), "embed-b/8");
}

#[test]
fn idle_unload_keeps_the_resident_capabilities() {
    let harness = Harness::all_devices();
    harness.embed(SECOND).unwrap();
    harness.describe(SECOND).unwrap();
    assert_eq!(*harness.onnx.loads.lock().unwrap(), 1);
    assert_eq!(*harness.llama.loads.lock().unwrap(), 1);
    assert_eq!(
        harness
            .router
            .unload_idle(Duration::ZERO, &[Capability::Embed]),
        1
    );
    harness.embed(SECOND).unwrap();
    harness.describe(SECOND).unwrap();
    assert_eq!(*harness.onnx.loads.lock().unwrap(), 1);
    assert_eq!(*harness.llama.loads.lock().unwrap(), 2);
}
