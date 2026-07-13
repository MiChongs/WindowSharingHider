mod affinity;
mod enumeration;
mod icons;
mod remote;
mod resources;

use crate::model::{Affinity, AffinityApplyOutcome, WindowSnapshot};
use crate::platform::{AffinityTarget, PlatformResult, ScanOptions, WindowPlatform};

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsPlatform;

impl WindowPlatform for WindowsPlatform {
    fn enumerate(&self, options: ScanOptions) -> PlatformResult<Vec<WindowSnapshot>> {
        enumeration::enumerate_windows(options)
    }

    fn apply_affinity(
        &self,
        target: AffinityTarget,
        affinity: Affinity,
    ) -> PlatformResult<AffinityApplyOutcome> {
        affinity::apply_window_affinity(target, affinity)
    }
}
