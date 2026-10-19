//! Event fan-out to the interface (Tauri events or the dev bridge's SSE).

use sonora_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

pub type Listener = Box<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Default)]
pub struct EventBus {
    listeners: RwLock<Vec<(u64, Listener)>>,
    next: AtomicU64,
}

impl EventBus {
    pub fn subscribe(&self, l: Listener) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.listeners.write().unwrap_or_else(|p| p.into_inner()).push((id, l));
        id
    }

    pub fn unsubscribe(&self, id: u64) {
        self.listeners.write().unwrap_or_else(|p| p.into_inner()).retain(|(i, _)| *i != id);
    }

    pub fn emit(&self, name: &str, payload: &Value) {
        let text = payload.to_string_compact();
        for (_, l) in self.listeners.read().unwrap_or_else(|p| p.into_inner()).iter() {
            l(name, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn subscribe_emit_unsubscribe() {
        let bus = EventBus::default();
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let id = bus.subscribe(Box::new(move |n, p| g.lock().unwrap().push(format!("{n}:{p}"))));
        bus.emit("x", &Value::Int(1));
        bus.unsubscribe(id);
        bus.emit("x", &Value::Int(2));
        assert_eq!(*got.lock().unwrap(), vec!["x:1".to_string()]);
    }
}
