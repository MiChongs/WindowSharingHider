use crate::model::{
    Affinity, AffinityApplyOutcome, OperationError, WindowKey, WindowMetadata, WindowSnapshot,
};

pub mod windows;

pub type PlatformResult<T> = Result<T, OperationError>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanOptions {
    pub include_system_candidates: bool,
    pub include_wetype_candidate: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffinityTarget {
    pub key: WindowKey,
    pub metadata: WindowMetadata,
}

impl From<WindowMetadata> for AffinityTarget {
    fn from(metadata: WindowMetadata) -> Self {
        Self {
            key: metadata.key,
            metadata,
        }
    }
}

pub trait WindowPlatform: Send + Sync + 'static {
    fn enumerate(&self, options: ScanOptions) -> PlatformResult<Vec<WindowSnapshot>>;

    fn apply_affinity(
        &self,
        target: AffinityTarget,
        affinity: Affinity,
    ) -> PlatformResult<AffinityApplyOutcome>;
}
