use context_infer::Device;
use llama_cpp_2::{LlamaBackendDevice, LlamaBackendDeviceType, list_llama_ggml_backend_devices};

/// The ggml device indices for `device`: the one with the most memory of that backend,
/// discrete before integrated. Empty for the CPU; `None` when the backend module did not
/// load or has no device.
pub(crate) fn device_indices(device: Device) -> Option<Vec<usize>> {
    let devices = list_llama_ggml_backend_devices();
    if device == Device::Cpu {
        return best_device(&devices, "CPU").map(|_| Vec::new());
    }
    let backend = ggml_backend_name(device)?;
    best_device(&devices, backend).map(|index| vec![index])
}

const fn ggml_backend_name(device: Device) -> Option<&'static str> {
    match device {
        Device::Cuda => Some("CUDA"),
        Device::Vulkan => Some("Vulkan"),
        Device::Rocm => Some("ROCm"),
        _ => None,
    }
}

fn best_device(devices: &[LlamaBackendDevice], backend: &str) -> Option<usize> {
    devices
        .iter()
        .filter(|device| device.backend == backend)
        .max_by_key(|device| {
            (
                device.device_type == LlamaBackendDeviceType::Gpu,
                device.memory_total,
            )
        })
        .map(|device| device.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(
        index: usize,
        backend: &str,
        kind: LlamaBackendDeviceType,
        gib: usize,
    ) -> LlamaBackendDevice {
        LlamaBackendDevice {
            index,
            name: format!("{backend}{index}"),
            description: String::new(),
            backend: backend.into(),
            memory_total: gib << 30,
            memory_free: gib << 30,
            device_type: kind,
        }
    }

    #[test]
    fn prefers_the_discrete_gpu_of_the_backend() {
        let devices = [
            device(0, "Vulkan", LlamaBackendDeviceType::IntegratedGpu, 16),
            device(1, "Vulkan", LlamaBackendDeviceType::Gpu, 6),
            device(2, "CUDA", LlamaBackendDeviceType::Gpu, 6),
            device(3, "CPU", LlamaBackendDeviceType::Cpu, 32),
        ];
        assert_eq!(best_device(&devices, "Vulkan"), Some(1));
        assert_eq!(best_device(&devices, "CUDA"), Some(2));
        assert_eq!(best_device(&devices, "ROCm"), None);
    }
}
