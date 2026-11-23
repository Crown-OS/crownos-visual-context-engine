use std::path::Path;

use context_infer::Device;
use crownos_config::system::ai::AiConfig;
use ort::ep::{self, ExecutionProviderDispatch};

use crate::util::cache_dir::cache_dir;

/// Shared by every provider library ONNX Runtime loads on demand.
const SHARED_PROVIDER_LIBRARY: &str = "shared";

/// Stems of the `libonnxruntime_providers_<stem>.so` files that `device` needs next to
/// `libonnxruntime.so`; `None` when ONNX Runtime does not run on it.
const fn library_stems(device: Device) -> Option<&'static [&'static str]> {
    match device {
        Device::TensorRt => Some(&[SHARED_PROVIDER_LIBRARY, "tensorrt", "cuda"]),
        Device::Cuda => Some(&[SHARED_PROVIDER_LIBRARY, "cuda"]),
        Device::MiGraphX => Some(&[SHARED_PROVIDER_LIBRARY, "migraphx"]),
        Device::OpenVinoNpu | Device::OpenVinoGpu => Some(&[SHARED_PROVIDER_LIBRARY, "openvino"]),
        Device::VitisAi => Some(&[SHARED_PROVIDER_LIBRARY, "vitisai"]),
        Device::Cpu => Some(&[]),
        _ => None,
    }
}

/// Whether `device` can work here: the installer set it up (when `ai.ron` exists) and its
/// provider libraries are installed. No session or model is created.
pub(crate) fn device_is_usable(
    device: Device,
    library: &Path,
    installed: Option<&AiConfig>,
) -> bool {
    let set_up = device == Device::Cpu
        || installed.is_none_or(|config| config.device_rank(device).is_some());
    let directory = library.parent().unwrap_or(Path::new("/"));
    set_up
        && library_stems(device).is_some_and(|stems| {
            stems.iter().all(|stem| {
                directory
                    .join(format!("libonnxruntime_providers_{stem}.so"))
                    .is_file()
            })
        })
}

/// The registrations that run a graph on `device`. Each fails the session instead of
/// falling back in silence, so the router can try the next candidate.
pub(crate) fn execution_providers(device: Device) -> Vec<ExecutionProviderDispatch> {
    let cache = |label: &str| cache_dir(label);
    let registrations = match device {
        Device::TensorRt => {
            let tensor_rt = ep::TensorRT::default();
            let tensor_rt = match cache("tensorrt") {
                Some(directory) => tensor_rt
                    .with_engine_cache(true)
                    .with_engine_cache_path(&directory)
                    .with_timing_cache(true)
                    .with_timing_cache_path(directory),
                None => tensor_rt,
            };
            vec![tensor_rt.build(), ep::CUDA::default().build()]
        }
        Device::Cuda => vec![ep::CUDA::default().build()],
        Device::MiGraphX => vec![ep::MIGraphX::default().build()],
        Device::OpenVinoNpu => vec![openvino("NPU", cache("openvino-npu")).build()],
        Device::OpenVinoGpu => vec![openvino("GPU", cache("openvino-gpu")).build()],
        Device::VitisAi => vec![
            match cache("vitisai") {
                Some(directory) => ep::Vitis::default().with_cache_dir(directory),
                None => ep::Vitis::default(),
            }
            .build(),
        ],
        _ => Vec::new(),
    };
    registrations
        .into_iter()
        .map(ExecutionProviderDispatch::error_on_failure)
        .collect()
}

fn openvino(device_type: &str, cache: Option<String>) -> ep::OpenVINO {
    let openvino = ep::OpenVINO::default().with_device_type(device_type);
    match cache {
        Some(directory) => openvino.with_cache_dir(directory),
        None => openvino,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_needs_no_provider_library_and_vulkan_is_not_ours() {
        let library = Path::new("/nonexistent/libonnxruntime.so");
        assert!(device_is_usable(Device::Cpu, library, None));
        assert!(!device_is_usable(Device::Cuda, library, None));
        assert!(!device_is_usable(Device::Vulkan, library, None));
    }

    #[test]
    fn a_device_the_installer_did_not_set_up_is_not_usable() {
        let installed = AiConfig {
            devices: vec![Device::Cpu],
            ..AiConfig::default()
        };
        let directory = std::env::temp_dir().join(format!("ort-probe-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        for stem in ["shared", "cuda"] {
            std::fs::write(
                directory.join(format!("libonnxruntime_providers_{stem}.so")),
                b"",
            )
            .unwrap();
        }
        let library = directory.join("libonnxruntime.so");
        assert!(device_is_usable(Device::Cuda, &library, None));
        assert!(!device_is_usable(Device::Cuda, &library, Some(&installed)));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tensor_rt_keeps_cuda_for_the_nodes_it_rejects() {
        assert_eq!(execution_providers(Device::TensorRt).len(), 2);
        assert!(execution_providers(Device::Cpu).is_empty());
    }
}
