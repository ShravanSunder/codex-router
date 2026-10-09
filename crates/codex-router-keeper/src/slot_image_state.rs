use crate::{ImageError, ImageLease};
use codex_router_keeper_protocol::{ComponentFingerprint, ComponentKind};
/// Local image commit input only; the real supervisor must establish the release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageCommitRelease {
    Deactivated,
    ForcedPredicateCompleted,
}
pub struct SlotImageState {
    kind: ComponentKind,
    committed: ImageLease,
    candidate: Option<ImageLease>,
}
impl SlotImageState {
    pub fn new(kind: ComponentKind, committed: ImageLease) -> Self {
        Self {
            kind,
            committed,
            candidate: None,
        }
    }
    pub fn committed(&self) -> &ImageLease {
        &self.committed
    }
    pub fn candidate(&self) -> Option<&ImageLease> {
        self.candidate.as_ref()
    }
    pub fn committed_fingerprint(&self) -> ComponentFingerprint {
        self.committed.fingerprint(self.kind)
    }
    pub fn prepare_candidate(&mut self, candidate: ImageLease) -> Result<(), ImageError> {
        if self.candidate.is_some() {
            return Err(ImageError::CandidateState);
        }
        self.candidate = Some(candidate);
        Ok(())
    }
    pub fn refuse_candidate(&mut self) -> Result<ImageLease, ImageError> {
        self.candidate.take().ok_or(ImageError::CandidateState)
    }
    pub fn commit_candidate(
        &mut self,
        _release: ImageCommitRelease,
    ) -> Result<ImageLease, ImageError> {
        let incoming = self.candidate.take().ok_or(ImageError::CandidateState)?;
        Ok(std::mem::replace(&mut self.committed, incoming))
    }
}
