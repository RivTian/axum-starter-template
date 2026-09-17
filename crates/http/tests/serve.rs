//! `serve` 的行为测试：真实 socket、真实请求、真实关停。

use std::time::Duration;

use {{crate_prefix_snake}}_http::{router, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

#[tokio::test]
async fn serves_the_router_and_stops_on_shutdown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let address = listener.local_addr().expect("本地地址");
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        serve(listener, router(), async move {
            let _ = stop_rx.await;
        })
        .await
    });

    // 零路由：请求得到 404，但"服务确实在跑"这件事必须成立。
    let mut stream = TcpStream::connect(address).await.expect("连接");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n")
        .await
        .expect("写请求");
    let mut buffer = vec![0u8; 1024];
    let read = stream.read(&mut buffer).await.expect("读响应");
    let status = String::from_utf8_lossy(&buffer[..read])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        status.starts_with("HTTP/1.1 404"),
        "零路由应当回 404：{status}"
    );
    drop(stream);

    stop_tx.send(()).expect("发关停信号");
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("关停不应超时")
        .expect("serve 任务不应 panic");
    assert!(result.is_ok(), "优雅关停应当返回 Ok：{result:?}");
}

#[tokio::test]
async fn shutdown_before_any_connection_returns_ok() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        serve(listener, router(), async move {
            let _ = stop_rx.await;
        })
        .await
    });
    stop_tx.send(()).expect("发关停信号");
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("关停不应超时")
        .expect("serve 任务不应 panic");
    assert!(result.is_ok(), "{result:?}");
}
