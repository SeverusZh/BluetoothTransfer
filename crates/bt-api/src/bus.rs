use std::sync::Mutex;

use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::event::Event;

/// 多订阅者事件总线；订阅端断开后自动清理。
#[derive(Debug, Default)]
pub struct EventBus {
    subscribers: Mutex<Vec<Sender<Event>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// 每个订阅者持有独立通道，互不阻塞。
    pub fn subscribe(&self) -> Receiver<Event> {
        let (sender, receiver) = unbounded();
        let mut subscribers = self.subscribers.lock().expect("事件总线订阅表被投毒");
        subscribers.push(sender);
        receiver
    }

    /// 广播事件；已关闭的订阅者在此被剔除。
    pub fn emit(&self, event: Event) {
        let mut subscribers = self.subscribers.lock().expect("事件总线订阅表被投毒");
        subscribers.retain(|sender| sender.send(event.clone()).is_ok());
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().map(|list| list.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivers_to_every_subscriber_and_drops_closed_ones() {
        let bus = EventBus::new();
        let first = bus.subscribe();
        let second = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
        bus.emit(Event::QueueIdle);
        assert_eq!(first.try_recv().unwrap(), Event::QueueIdle);
        assert_eq!(second.try_recv().unwrap(), Event::QueueIdle);
        drop(second);
        bus.emit(Event::QueueIdle);
        assert_eq!(bus.subscriber_count(), 1);
    }
}
