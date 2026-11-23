use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crownos_config::system::ai::{Device, Runtime};
use serde::{Deserialize, Serialize};

use super::backend::RuntimeBackend;
use crate::registry::Candidate;
use crate::task::Task;

/// `$XDG_CACHE_HOME/crownos/context/backends.ron`.
pub fn default_benchmark_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|cache| cache.join("crownos").join("context").join("backends.ron"))
}

/// The versions a measurement depends on: one `runtime=version` part per backend.
pub fn benchmark_fingerprint(backends: &[Arc<dyn RuntimeBackend>]) -> String {
    let mut parts: Vec<String> = backends
        .iter()
        .map(|backend| format!("{:?}={}", backend.runtime(), backend.version()))
        .collect();
    parts.sort_unstable();
    parts.join(";")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Measurement {
    task: Task,
    model: String,
    runtime: Runtime,
    device: Device,
    nanos_per_unit: f64,
}

impl Measurement {
    fn is_for(&self, task: Task, candidate: &Candidate) -> bool {
        self.task == task
            && self.model == candidate.model
            && self.runtime == candidate.runtime
            && self.device == candidate.device
    }
}

/// Call latency per unit of work for each (task, model, runtime, device), valid for one
/// runtime and driver [`fingerprint`](Self::fingerprint). A unit is a pixel for OCR, a byte of
/// text for embedding and reranking, a sample for transcription and a token for generation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkCache {
    pub fingerprint: String,
    measurements: Vec<Measurement>,
}

impl BenchmarkCache {
    /// An empty cache when the file is missing, unreadable or from other versions.
    pub fn load(path: &Path, fingerprint: &str) -> Self {
        let cached = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| ron::from_str::<Self>(&text).ok())
            .filter(|cache| cache.fingerprint == fingerprint);
        cached.unwrap_or_else(|| Self {
            fingerprint: fingerprint.to_owned(),
            measurements: Vec::new(),
        })
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(io::Error::other)?;
        let partial = path.with_extension("ron.partial");
        std::fs::write(&partial, text)?;
        std::fs::rename(partial, path)
    }

    pub fn nanos_per_unit(&self, task: Task, candidate: &Candidate) -> Option<f64> {
        self.measurements
            .iter()
            .find(|measurement| measurement.is_for(task, candidate))
            .map(|measurement| measurement.nanos_per_unit)
    }

    /// The expected time of a call with `units` of work.
    pub fn latency(&self, task: Task, candidate: &Candidate, units: u64) -> Option<Duration> {
        let nanos = self.nanos_per_unit(task, candidate)? * units.max(1) as f64;
        Duration::try_from_secs_f64(nanos / 1e9).ok()
    }

    /// Adds a sample. Older samples keep three quarters of the weight.
    pub fn record(&mut self, task: Task, candidate: &Candidate, elapsed: Duration, units: u64) {
        let sample = elapsed.as_nanos() as f64 / units.max(1) as f64;
        match self
            .measurements
            .iter_mut()
            .find(|measurement| measurement.is_for(task, candidate))
        {
            Some(measurement) => {
                measurement.nanos_per_unit = measurement.nanos_per_unit * 0.75 + sample * 0.25;
            }
            None => self.measurements.push(Measurement {
                task,
                model: candidate.model.clone(),
                runtime: candidate.runtime,
                device: candidate.device,
                nanos_per_unit: sample,
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn candidate() -> Candidate {
        Candidate {
            model: "ocr".into(),
            runtime: Runtime::OnnxRuntime,
            device: Device::Cuda,
            on_battery: true,
            min_tier: crate::registry::Tier::Low,
        }
    }

    #[test]
    fn a_saved_cache_loads_only_for_the_same_versions() {
        let path = std::env::temp_dir()
            .join(format!("context-infer-bench-{}", std::process::id()))
            .join("backends.ron");
        let mut cache = BenchmarkCache::load(&path, "ort=1.23");
        cache.record(
            Task::RecognizeText,
            &candidate(),
            Duration::from_millis(8),
            1000,
        );
        cache.save(&path).unwrap();

        let same = BenchmarkCache::load(&path, "ort=1.23");
        assert_eq!(
            same.latency(Task::RecognizeText, &candidate(), 2000),
            Some(Duration::from_millis(16))
        );
        let upgraded = BenchmarkCache::load(&path, "ort=1.24");
        assert_eq!(upgraded.latency(Task::RecognizeText, &candidate(), 1), None);
        assert_eq!(upgraded.fingerprint, "ort=1.24");
    }

    #[test]
    fn new_samples_move_the_average_by_a_quarter() {
        let mut cache = BenchmarkCache::default();
        cache.record(
            Task::RecognizeText,
            &candidate(),
            Duration::from_millis(8),
            1,
        );
        cache.record(
            Task::RecognizeText,
            &candidate(),
            Duration::from_millis(16),
            1,
        );
        assert_eq!(
            cache.latency(Task::RecognizeText, &candidate(), 1),
            Some(Duration::from_millis(10))
        );
    }
}
