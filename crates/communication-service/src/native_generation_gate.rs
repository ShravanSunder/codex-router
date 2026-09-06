//! Serialized generation admission; every admitted relay shares its retirement signal.
use communication_protocol::CodexGeneration;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct NativeGenerationGate {
    state: Arc<Mutex<GenerationState>>,
}
#[derive(Default)]
struct GenerationState {
    last_generation: Option<CodexGeneration>,
    accepting: Option<NativeAdmission>,
}
#[derive(Clone)]
pub struct NativeAdmission {
    schemas: Option<Arc<codex_native_integration::NativePayloadSchemas>>,
    generation: CodexGeneration,
    backend_path: PathBuf,
    retirement: CancellationToken,
}
impl NativeAdmission {
    #[must_use]
    pub fn schemas(&self) -> Option<Arc<codex_native_integration::NativePayloadSchemas>> {
        self.schemas.clone()
    }
    #[must_use]
    pub fn generation(&self) -> &CodexGeneration {
        &self.generation
    }
    #[must_use]
    pub fn backend_path(&self) -> &Path {
        &self.backend_path
    }
    #[must_use]
    pub fn retirement(&self) -> CancellationToken {
        self.retirement.clone()
    }
}
impl NativeGenerationGate {
    pub fn activate(
        &self,
        generation: CodexGeneration,
        backend_path: PathBuf,
        schemas: Option<Arc<codex_native_integration::NativePayloadSchemas>>,
    ) -> io::Result<()> {
        if !backend_path.is_absolute() {
            return Err(io::Error::other("backend path must be absolute"));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("generation admission unavailable"))?;
        if state.accepting.is_some() {
            return Err(io::Error::other("retire the accepting generation first"));
        }
        if let Some(previous) = &state.last_generation
            && (previous.service_epoch != generation.service_epoch
                || u64::from(generation.generation) <= u64::from(previous.generation))
        {
            return Err(io::Error::other(
                "generation must advance within this service epoch",
            ));
        }
        state.last_generation = Some(generation.clone());
        state.accepting = Some(NativeAdmission {
            schemas,
            generation,
            backend_path,
            retirement: CancellationToken::new(),
        });
        Ok(())
    }
    pub fn acquire(&self) -> io::Result<NativeAdmission> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("generation admission unavailable"))?
            .accepting
            .clone()
            .ok_or_else(|| io::Error::other("native backend unavailable"))
    }
    pub fn retire(&self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("generation admission unavailable"))?;
        if let Some(admission) = state.accepting.take() {
            admission.retirement.cancel();
        }
        Ok(())
    }
}
impl Drop for GenerationState {
    fn drop(&mut self) {
        if let Some(admission) = &self.accepting {
            admission.retirement.cancel();
        }
    }
}
