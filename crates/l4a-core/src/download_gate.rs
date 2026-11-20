use std::sync::Mutex;
use tokio::sync::{Mutex as AsyncMutex, Notify};

pub(crate) struct DownloadGate {
    queue: AsyncMutex<()>,
    slots: Mutex<Slots>,
    available: Notify,
}

struct Slots {
    active: usize,
    limit: usize,
}

pub(crate) struct DownloadSlot<'a>(&'a DownloadGate);

impl DownloadGate {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            queue: AsyncMutex::new(()),
            slots: Mutex::new(Slots { active: 0, limit }),
            available: Notify::new(),
        }
    }

    pub(crate) fn set_limit(&self, limit: usize) {
        self.slots.lock().unwrap().limit = limit;
        self.available.notify_one();
    }

    pub(crate) async fn acquire(&self) -> DownloadSlot<'_> {
        // Only the head of the FIFO queue waits for capacity to change.
        let _turn = self.queue.lock().await;
        loop {
            {
                let mut slots = self.slots.lock().unwrap();
                if slots.active < slots.limit {
                    slots.active += 1;
                    return DownloadSlot(self);
                }
            }
            self.available.notified().await;
        }
    }
}

impl Drop for DownloadSlot<'_> {
    fn drop(&mut self) {
        self.0.slots.lock().unwrap().active -= 1;
        self.0.available.notify_one();
    }
}
