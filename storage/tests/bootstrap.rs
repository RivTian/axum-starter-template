//! 门面契约测试
//!
//! 钉住对上暴露的形状：`Storage` 必须能作为 `Arc<dyn Storage>` 注入到跨线程的
//! 运行时状态里，错误必须能无损上浮。这两点一旦被破坏（例如给 trait 加了泛型方法
//! 或非 `Send` 的返回值），上层接线会连锁失败，越早红越好。

use std::sync::Arc;

use {{crate_prefix_snake}}_core::AppError;
use {{crate_prefix_snake}}_storage::{Storage, StorageError};

/// trait 必须对象安全：上层持有的是 `Arc<dyn Storage>` 而非具体类型。
#[test]
fn storage_trait_is_object_safe() {
    fn accepts(_: &Arc<dyn Storage>) {}
    let _ = accepts;
}

/// 门面必须可跨线程共享：存储实例会被多个并发任务同时持有。
#[test]
fn storage_handle_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn Storage>();
    assert_send_sync::<Arc<dyn Storage>>();
}

/// 存储错误必须能无损上浮到应用级错误。
#[test]
fn storage_error_converts_to_app_error() {
    let app: AppError = StorageError::Init("boom".into()).into();
    assert!(matches!(app, AppError::Storage(_)));
}
