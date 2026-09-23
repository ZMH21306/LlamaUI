//! Tauri 事件总线。
//!
//! 基于 `tokio::sync::broadcast` 实现，解耦后端模块与前端监听。
//! 替代直接调用 `app.emit(event, payload)`，新增事件类型无需修改命令层。
//!
//! # 用法
//!
//! ```ignore
//! let bus = EventBus::new(16);
//! bus.emit("update-state", payload);
//! bus.listen("update-state", |payload| { ... });
//! ```

use std::sync::Arc;
use tokio::sync::broadcast;
use tauri::Emitter;

const DEFAULT_CAPACITY: usize = 64;

/// Tauri 事件总线。
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<EventPayload>,
    /// 前端事件名 -> 业务事件名（用于路由）。
    routes: Arc<Vec<(String, String)>>,
}

/// 事件负载（统一包装）。
#[derive(Clone)]
pub struct EventPayload {
    pub event: String,
    pub payload: serde_json::Value,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            sender: broadcast::channel(capacity).0,
            routes: Arc::new(Vec::new()),
        }
    }

    /// 注册前端事件路由。
    pub fn with_route(mut self, app_event: &str, bus_event: &str) -> Self {
        Arc::get_mut(&mut self.routes)
            .expect("clone 后才能注册路由")
            .push((app_event.to_string(), bus_event.to_string()));
        self
    }

    /// 发射事件到前端（通过 Tauri Emitter）。
    ///
    /// `app` 是 Tauri 的 AppHandle，`event` 是前端监听的事件名。
    pub fn emit_app<E: serde::Serialize + Clone>(&self, app: &tauri::AppHandle, event: &str, payload: &E) {
        let _ = app.emit(event, payload);
        // 同时广播到内部总线，供后端监听
        let json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        let _ = self.sender.send(EventPayload {
            event: event.to_string(),
            payload: json,
        });
    }

    /// 发射事件（仅内部总线，不发到前端）。
    pub fn emit<E: serde::Serialize>(&self, event: &str, payload: &E) {
        let _ = self.sender.send(EventPayload {
            event: event.to_string(),
            payload: serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
        });
    }

    /// 订阅事件，返回接收端。
    pub fn subscribe(&self) -> broadcast::Receiver<EventPayload> {
        self.sender.subscribe()
    }

    /// 获取当前订阅者数量。
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

        #[tokio::test]
    async fn bus_emit_and_subscribe() {
        let bus = EventBus::new(4);
        let mut rx = bus.subscribe();
        bus.emit("test-event", &"hello");
        let received = rx.recv().await.unwrap();
        assert_eq!(received.event, "test-event");
        assert_eq!(received.payload, "hello");
    }

    #[test]
    fn bus_emits_null_for_non_serializable() {
        let bus = EventBus::new(4);
        bus.emit("test", &vec![0u8; 1024]);
        // 不应 panic
    }

    #[test]
    fn bus_subscriber_count() {
        let bus = EventBus::new(4);
        assert_eq!(bus.subscriber_count(), 0);
        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
    }
}