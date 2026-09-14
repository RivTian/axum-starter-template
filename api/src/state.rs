use service_core::BuildInfo;
use service_core::lifecycle::LifecycleHandle;
use service_storage::Storage;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub lifecycle: LifecycleHandle,
    pub build: BuildInfo,
}
