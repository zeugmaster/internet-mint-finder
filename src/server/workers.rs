//! Worker progress is independent of whether a remote mint succeeds or has work.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};

struct Worker {
    at: Instant,
    deadline: Duration,
    ready: bool,
}
#[derive(Default)]
pub struct Workers(Mutex<BTreeMap<&'static str, Worker>>);
#[derive(Serialize)]
pub struct WorkerStatus {
    pub name: &'static str,
    pub ready: bool,
    pub stalled: bool,
    pub seconds_since_progress: u64,
}
impl Workers {
    pub fn register(&self, name: &'static str, deadline: Duration, ready: bool) {
        self.0.lock().unwrap().insert(
            name,
            Worker {
                at: Instant::now(),
                deadline,
                ready,
            },
        );
    }
    pub fn beat(&self, name: &'static str) {
        if let Some(worker) = self.0.lock().unwrap().get_mut(name) {
            worker.at = Instant::now();
        }
    }
    pub fn ready(&self, name: &'static str, ready: bool) {
        if let Some(worker) = self.0.lock().unwrap().get_mut(name) {
            worker.ready = ready;
        }
    }
    pub fn status(&self) -> Vec<WorkerStatus> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(name, worker)| WorkerStatus {
                name,
                ready: worker.ready,
                stalled: worker.at.elapsed() > worker.deadline,
                seconds_since_progress: worker.at.elapsed().as_secs(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_is_separate_from_a_stalled_worker() {
        let workers = Workers::default();
        workers.register("relay", Duration::from_secs(60), false);
        assert!(!workers.status()[0].ready);
        assert!(!workers.status()[0].stalled);
        workers.0.lock().unwrap().get_mut("relay").unwrap().at =
            Instant::now() - Duration::from_secs(61);
        assert!(workers.status()[0].stalled);
        workers.beat("relay");
        workers.ready("relay", true);
        assert!(workers.status()[0].ready && !workers.status()[0].stalled);
    }
}
