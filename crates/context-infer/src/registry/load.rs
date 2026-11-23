use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crownos_config::system::ai::{Device, Runtime};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{Candidate, Capability, ModelEntry, REGISTRY_VERSION, Registry};
use crate::task::Task;

pub const SYSTEM_REGISTRY_PATH: &str = "/usr/share/crownos/context/registry.ron";

/// `$XDG_CONFIG_HOME/crownos/models.ron`: models and task lists that replace the system ones.
pub fn user_override_path() -> Option<PathBuf> {
    dirs::config_dir().map(|config| config.join("crownos").join("models.ron"))
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("could not parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<ron::error::SpannedError>,
    },
    #[error("the registry has version {found}, not {REGISTRY_VERSION}")]
    Version { found: u32 },
    #[error("{task:?} names the unknown model {model}")]
    UnknownModel { task: Task, model: String },
    #[error("{model} has no artifact for {runtime:?}")]
    MissingArtifact { model: String, runtime: Runtime },
    #[error("{runtime:?} does not run on {device:?}")]
    UnsupportedDevice { runtime: Runtime, device: Device },
    #[error("{model} cannot do {capability:?}, which {task:?} needs")]
    MissingCapability {
        task: Task,
        model: String,
        capability: Capability,
    },
    #[error("the embedding model {model} has no embedding_dimensions")]
    MissingDimensions { model: String },
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryOverride {
    #[serde(default)]
    models: BTreeMap<String, ModelEntry>,
    #[serde(default)]
    tasks: BTreeMap<Task, Vec<Candidate>>,
}

impl Registry {
    /// The system catalog with the user's override file on top, when that file exists. A
    /// user file that does not parse or validate is logged and ignored.
    pub fn load(system: &Path, user_override: Option<&Path>) -> Result<Self, RegistryError> {
        let registry: Self = read_ron(system)?.ok_or_else(|| RegistryError::Read {
            path: system.to_owned(),
            source: io::ErrorKind::NotFound.into(),
        })?;
        registry.validate()?;
        let Some(user_override) = user_override else {
            return Ok(registry);
        };
        match registry.with_override(user_override) {
            Ok(Some(changed)) => Ok(changed),
            Ok(None) => Ok(registry),
            Err(error) => {
                tracing::warn!(%error, "ignoring the user model registry");
                Ok(registry)
            }
        }
    }

    fn with_override(&self, path: &Path) -> Result<Option<Self>, RegistryError> {
        let Some(changes) = read_ron::<RegistryOverride>(path)? else {
            return Ok(None);
        };
        let mut changed = self.clone();
        changed.models.extend(changes.models);
        changed.tasks.extend(changes.tasks);
        changed.validate()?;
        Ok(Some(changed))
    }

    pub fn from_ron(text: &str) -> Result<Self, RegistryError> {
        let registry: Self = ron::from_str(text).map_err(|source| RegistryError::Parse {
            path: PathBuf::from("<text>"),
            source: Box::new(source),
        })?;
        registry.validate()?;
        Ok(registry)
    }

    fn validate(&self) -> Result<(), RegistryError> {
        if self.version != REGISTRY_VERSION {
            return Err(RegistryError::Version {
                found: self.version,
            });
        }
        for (&task, candidates) in &self.tasks {
            for candidate in candidates {
                validate_candidate(self, task, candidate)?;
            }
        }
        Ok(())
    }
}

fn validate_candidate(
    registry: &Registry,
    task: Task,
    candidate: &Candidate,
) -> Result<(), RegistryError> {
    let model =
        registry
            .models
            .get(&candidate.model)
            .ok_or_else(|| RegistryError::UnknownModel {
                task,
                model: candidate.model.clone(),
            })?;
    if let Some(&capability) = task
        .required_capabilities()
        .iter()
        .find(|&&capability| !model.has(capability))
    {
        return Err(RegistryError::MissingCapability {
            task,
            model: candidate.model.clone(),
            capability,
        });
    }
    if task == Task::EmbedText && model.embedding_dimensions.is_none() {
        return Err(RegistryError::MissingDimensions {
            model: candidate.model.clone(),
        });
    }
    if model.artifact_for(candidate.runtime).is_none() {
        return Err(RegistryError::MissingArtifact {
            model: candidate.model.clone(),
            runtime: candidate.runtime,
        });
    }
    if !candidate.runtime.supports(candidate.device) {
        return Err(RegistryError::UnsupportedDevice {
            runtime: candidate.runtime,
            device: candidate.device,
        });
    }
    Ok(())
}

/// `None` when the file does not exist.
fn read_ron<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, RegistryError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RegistryError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    ron::from_str(&text)
        .map(Some)
        .map_err(|source| RegistryError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })
}
