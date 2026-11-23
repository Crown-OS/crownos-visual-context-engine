use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use context_infer::registry::{SYSTEM_REGISTRY_PATH, user_override_path};
use context_infer::{
    BenchmarkCache, Device, Registry, RoutedTasks, Router, RouterSettings, RuntimeBackend,
    benchmark_fingerprint, default_benchmark_path,
};
use context_infer_llama::{DEFAULT_GGML_MODULES_DIR, LlamaBackendHost};
use context_infer_ort::OrtBackend;

/// The router and the task layer over the local backends. Remote candidates are removed:
/// this process has no network, and images never leave the machine.
#[derive(Debug)]
pub(crate) struct Engine {
    pub(crate) router: Arc<Router>,
    pub(crate) tasks: RoutedTasks,
    benchmark_path: Option<PathBuf>,
}

impl Engine {
    pub(crate) fn start(
        registry_path: Option<&Path>,
        models_dir: PathBuf,
        ggml_modules: Option<&Path>,
    ) -> Result<Self> {
        let registry_path = registry_path.unwrap_or(Path::new(SYSTEM_REGISTRY_PATH));
        let mut registry = Registry::load(registry_path, user_override_path().as_deref())
            .with_context(|| format!("reading {}", registry_path.display()))?;
        for candidates in registry.tasks.values_mut() {
            candidates.retain(|candidate| candidate.device != Device::Remote);
        }
        let backends = local_backends(ggml_modules);
        let fingerprint = benchmark_fingerprint(&backends);
        let benchmark_path = default_benchmark_path();
        let benchmarks = benchmark_path
            .as_deref()
            .map(|path| BenchmarkCache::load(path, &fingerprint))
            .unwrap_or_default();
        let mut settings = RouterSettings::new(models_dir);
        settings.benchmarks = benchmarks;
        let router = Arc::new(Router::new(registry, backends, settings));
        Ok(Self {
            tasks: RoutedTasks::new(Arc::clone(&router)),
            router,
            benchmark_path,
        })
    }

    pub(crate) fn save_benchmarks(&self) {
        let Some(path) = &self.benchmark_path else {
            return;
        };
        if let Err(error) = self.router.benchmarks().save(path) {
            tracing::warn!(%error, path = %path.display(), "saving the latency cache");
        }
    }
}

fn local_backends(ggml_modules: Option<&Path>) -> Vec<Arc<dyn RuntimeBackend>> {
    let mut backends: Vec<Arc<dyn RuntimeBackend>> = vec![Arc::new(OrtBackend::new())];
    let installed_modules = Path::new(DEFAULT_GGML_MODULES_DIR);
    let modules = ggml_modules.or_else(|| installed_modules.is_dir().then_some(installed_modules));
    match LlamaBackendHost::new(modules) {
        Ok(host) => backends.push(Arc::new(host)),
        Err(error) => tracing::warn!(%error, "llama.cpp is not usable"),
    }
    backends
}
