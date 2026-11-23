use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crownos_config::system::ai::{Device, Runtime};

use super::super::{LoadSpec, Router, RouterSettings, RuntimeBackend};
use crate::capability::{
    Embed, EmbedPurpose, EmbedSpace, Embeddings, Generate, GenerateCapabilities,
};
use crate::error::InferError;
use crate::registry::Registry;
use crate::request::{GenRequest, Generation, Job, Output, Priority, Sampling};
use crate::task::Task;

pub(super) const SECOND: Duration = Duration::from_secs(1);

pub(super) const REGISTRY: &str = r#"(
    version: 2,
    models: {
        "big": (capabilities: [Generate, Vision, JsonOutput], licence: "Apache-2.0",
                artifacts: [Gguf(model: (path: "big.gguf"), quant: "Q4_K_M"), Remote(name: "big")]),
        "small": (capabilities: [Generate, Vision, JsonOutput], licence: "Apache-2.0",
                  artifacts: [Gguf(model: (path: "small.gguf"), quant: "Q4_K_M")]),
        "embed-a": (capabilities: [Embed], licence: "Apache-2.0", embedding_dimensions: Some(4),
                    artifacts: [Onnx(graphs: {"encoder": (path: "a.onnx")}, quant: "int8"),
                                Gguf(model: (path: "a.gguf"), quant: "Q8_0")]),
        "embed-b": (capabilities: [Embed], licence: "MIT", embedding_dimensions: Some(8),
                    artifacts: [Onnx(graphs: {"encoder": (path: "b.onnx")}, quant: "int8")]),
    },
    tasks: {
        DescribeScreen: [
            (model: "big", runtime: LlamaCpp, device: Cuda, min_tier: High),
            (model: "small", runtime: LlamaCpp, device: Cuda),
            (model: "small", runtime: LlamaCpp, device: Vulkan),
            (model: "small", runtime: LlamaCpp, device: Cpu, on_battery: false),
            (model: "big", runtime: OpenAiCompatible, device: Remote),
        ],
        SummarizeSegments: [
            (model: "small", runtime: LlamaCpp, device: Cuda),
            (model: "small", runtime: LlamaCpp, device: Vulkan),
            (model: "big", runtime: LlamaCpp, device: Cuda),
            (model: "small", runtime: LlamaCpp, device: Cpu),
            (model: "big", runtime: OpenAiCompatible, device: Remote),
        ],
        EmbedText: [
            (model: "embed-a", runtime: OnnxRuntime, device: Cuda),
            (model: "embed-a", runtime: OnnxRuntime, device: Cpu),
            (model: "embed-a", runtime: LlamaCpp, device: Vulkan),
            (model: "embed-b", runtime: OnnxRuntime, device: Cpu),
        ],
    },
)"#;

type Behaviours = Arc<Mutex<HashMap<(String, Device), Behaviour>>>;

#[derive(Clone, Copy, Debug)]
pub(super) enum Behaviour {
    Answer,
    Fail,
    Busy,
    OwnTimeout,
    Hang,
    /// Takes one millisecond per text.
    SlowPerText,
    /// Takes 300 ms.
    Slow,
}

/// One fake per runtime. Calls are logged as `model@Device`.
#[derive(Debug)]
pub(super) struct FakeRuntime {
    runtime: Runtime,
    devices: Vec<Device>,
    behaviours: Behaviours,
    log: Arc<Mutex<Vec<String>>>,
    loads: Mutex<HashMap<Device, usize>>,
    load_times: Mutex<HashMap<Device, Duration>>,
}

impl FakeRuntime {
    fn new(runtime: Runtime, devices: &[Device], log: &Arc<Mutex<Vec<String>>>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            devices: devices.to_vec(),
            behaviours: Behaviours::default(),
            log: Arc::clone(log),
            loads: Mutex::default(),
            load_times: Mutex::default(),
        })
    }

    pub(super) fn set(&self, model: &str, device: Device, behaviour: Behaviour) {
        self.behaviours
            .lock()
            .unwrap()
            .insert((model.to_owned(), device), behaviour);
    }

    /// Loads on every device.
    pub(super) fn load_count(&self) -> usize {
        self.loads.lock().unwrap().values().sum()
    }

    pub(super) fn loads_on(&self, device: Device) -> usize {
        self.loads
            .lock()
            .unwrap()
            .get(&device)
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn set_load_time(&self, device: Device, load_time: Duration) {
        self.load_times.lock().unwrap().insert(device, load_time);
    }

    fn fake_model(&self, spec: &LoadSpec<'_>) -> FakeModel {
        let load_time = self.load_times.lock().unwrap().get(&spec.device).copied();
        thread::sleep(load_time.unwrap_or_default());
        *self.loads.lock().unwrap().entry(spec.device).or_default() += 1;
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
    fn behave(&self, texts: usize) -> Result<(), InferError> {
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
                thread::sleep(Duration::from_secs(30));
                Ok(())
            }
            Behaviour::Slow => {
                thread::sleep(Duration::from_millis(300));
                Ok(())
            }
            Behaviour::SlowPerText => {
                thread::sleep(Duration::from_millis(texts as u64));
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
        self.behave(1)?;
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
        self.behave(texts.len())?;
        let dimensions = self.space().dimensions as usize;
        Ok(Embeddings {
            dimensions,
            values: vec![0.5; dimensions * texts.len()],
        })
    }
}

pub(super) struct Harness {
    pub(super) router: Router,
    pub(super) llama: Arc<FakeRuntime>,
    pub(super) onnx: Arc<FakeRuntime>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    pub(super) fn new(
        llama_devices: &[Device],
        onnx_devices: &[Device],
        settings: RouterSettings,
    ) -> Self {
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

    pub(super) fn all_devices() -> Self {
        Self::new(
            &[Device::Cuda, Device::Vulkan, Device::Cpu],
            &[Device::Cuda, Device::Cpu],
            settings(),
        )
    }

    pub(super) fn describe(&self, budget: Duration) -> Result<String, InferError> {
        self.describe_job(Job::within(budget, Priority::Interactive))
    }

    pub(super) fn describe_job(&self, job: Job) -> Result<String, InferError> {
        let request = GenRequest {
            messages: Vec::new(),
            media: Vec::new(),
            output: Output::Text,
            sampling: Sampling::default(),
            max_tokens: 16,
            job,
        };
        self.router
            .generate(Task::DescribeScreen, request)
            .map(|generation| generation.text)
    }

    pub(super) fn embed_texts(
        &self,
        texts: Vec<String>,
        budget: Duration,
    ) -> Result<String, InferError> {
        self.router
            .embed(
                texts,
                EmbedPurpose::Document,
                &Job::within(budget, Priority::Background),
            )
            .map(|embedded| embedded.space.to_string())
    }

    pub(super) fn embed(&self, budget: Duration) -> Result<String, InferError> {
        self.embed_texts(vec!["a".into(), "b".into()], budget)
    }

    pub(super) fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// (model, device) of each ranked candidate.
    pub(super) fn ranked(&self, task: Task) -> Vec<String> {
        self.router
            .ranked(task)
            .iter()
            .map(|candidate| format!("{}@{:?}", candidate.model, candidate.device))
            .collect()
    }
}

pub(super) fn settings() -> RouterSettings {
    let mut settings = RouterSettings::new("/nonexistent/models".into());
    settings.watchdog_grace = Duration::from_millis(20);
    settings
}
