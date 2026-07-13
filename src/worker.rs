use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::model::{
    Affinity, AffinityApplyOutcome, OperationError, OperationStage, WindowKey, WindowSnapshot,
};
use crate::platform::{AffinityTarget, PlatformResult, ScanOptions, WindowPlatform};

#[derive(Clone, Debug)]
pub enum WorkerEvent {
    Snapshot {
        generation: u64,
        result: PlatformResult<Vec<WindowSnapshot>>,
    },
    AffinityResult {
        request_id: u64,
        key: WindowKey,
        requested: Affinity,
        result: PlatformResult<AffinityApplyOutcome>,
    },
}

#[derive(Clone, Debug)]
enum ScanCommand {
    Scan {
        generation: u64,
        options: ScanOptions,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
enum AffinityCommand {
    Apply {
        request_id: u64,
        target: AffinityTarget,
        affinity: Affinity,
    },
    Shutdown,
}

pub struct WorkerRuntime {
    scan_sender: Option<Sender<ScanCommand>>,
    affinity_sender: Option<Sender<AffinityCommand>>,
    event_receiver: Receiver<WorkerEvent>,
    scan_thread: Option<JoinHandle<()>>,
    affinity_thread: Option<JoinHandle<()>>,
}

impl WorkerRuntime {
    pub fn spawn(platform: Arc<dyn WindowPlatform>) -> Result<Self, OperationError> {
        let (scan_sender, scan_receiver) = mpsc::channel();
        let (affinity_sender, affinity_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();

        let scan_platform = Arc::clone(&platform);
        let scan_events = event_sender.clone();
        let scan_thread = thread::Builder::new()
            .name("window-scan-worker".into())
            .spawn(move || scan_worker(scan_platform, scan_receiver, scan_events))
            .map_err(|error| {
                OperationError::new(
                    OperationStage::EnumerateWindows,
                    format!("无法启动窗口扫描线程：{error}"),
                )
            })?;

        let affinity_thread = match thread::Builder::new()
            .name("window-affinity-worker".into())
            .spawn(move || affinity_worker(platform, affinity_receiver, event_sender))
        {
            Ok(thread) => thread,
            Err(error) => {
                let _ = scan_sender.send(ScanCommand::Shutdown);
                let _ = scan_thread.join();
                return Err(OperationError::new(
                    OperationStage::CreateRemoteThread,
                    format!("无法启动 affinity 工作线程：{error}"),
                ));
            }
        };

        Ok(Self {
            scan_sender: Some(scan_sender),
            affinity_sender: Some(affinity_sender),
            event_receiver,
            scan_thread: Some(scan_thread),
            affinity_thread: Some(affinity_thread),
        })
    }

    pub fn request_scan(
        &self,
        generation: u64,
        options: ScanOptions,
    ) -> Result<(), OperationError> {
        self.scan_sender
            .as_ref()
            .ok_or_else(shutdown_error)?
            .send(ScanCommand::Scan {
                generation,
                options,
            })
            .map_err(|_| shutdown_error())
    }

    pub fn request_affinity(
        &self,
        request_id: u64,
        target: AffinityTarget,
        affinity: Affinity,
    ) -> Result<(), OperationError> {
        self.affinity_sender
            .as_ref()
            .ok_or_else(shutdown_error)?
            .send(AffinityCommand::Apply {
                request_id,
                target,
                affinity,
            })
            .map_err(|_| shutdown_error())
    }

    pub fn try_recv(&self) -> Result<Option<WorkerEvent>, OperationError> {
        match self.event_receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(shutdown_error()),
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(sender) = self.scan_sender.take() {
            let _ = sender.send(ScanCommand::Shutdown);
        }
        if let Some(sender) = self.affinity_sender.take() {
            let _ = sender.send(AffinityCommand::Shutdown);
        }
        if let Some(thread) = self.scan_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.affinity_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WorkerRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn scan_worker(
    platform: Arc<dyn WindowPlatform>,
    receiver: Receiver<ScanCommand>,
    events: Sender<WorkerEvent>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            ScanCommand::Scan {
                generation,
                options,
            } => {
                let result = platform.enumerate(options);
                if events
                    .send(WorkerEvent::Snapshot { generation, result })
                    .is_err()
                {
                    break;
                }
            }
            ScanCommand::Shutdown => break,
        }
    }
}

fn affinity_worker(
    platform: Arc<dyn WindowPlatform>,
    receiver: Receiver<AffinityCommand>,
    events: Sender<WorkerEvent>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            AffinityCommand::Apply {
                request_id,
                target,
                affinity,
            } => {
                let key = target.key;
                let result = platform.apply_affinity(target, affinity);
                if events
                    .send(WorkerEvent::AffinityResult {
                        request_id,
                        key,
                        requested: affinity,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
            AffinityCommand::Shutdown => break,
        }
    }
}

fn shutdown_error() -> OperationError {
    OperationError::new(OperationStage::Shutdown, "后台运行时已经关闭")
}

#[cfg(test)]
mod tests {
    use std::thread::ThreadId;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;

    use super::*;
    use crate::model::{WindowHandle, WindowKind, WindowMetadata};

    #[derive(Default)]
    struct FakeState {
        scan_threads: Vec<ThreadId>,
        affinity_threads: Vec<ThreadId>,
        applied: Vec<Affinity>,
        fail_scan: bool,
    }

    #[derive(Default)]
    struct FakePlatform {
        state: Mutex<FakeState>,
    }

    impl WindowPlatform for FakePlatform {
        fn enumerate(&self, _options: ScanOptions) -> PlatformResult<Vec<WindowSnapshot>> {
            let mut state = self.state.lock();
            state.scan_threads.push(thread::current().id());
            if state.fail_scan {
                Err(OperationError::new(
                    OperationStage::EnumerateWindows,
                    "fake scan failure",
                ))
            } else {
                Ok(Vec::new())
            }
        }

        fn apply_affinity(
            &self,
            target: AffinityTarget,
            affinity: Affinity,
        ) -> PlatformResult<AffinityApplyOutcome> {
            let mut state = self.state.lock();
            state.affinity_threads.push(thread::current().id());
            state.applied.push(affinity);
            Ok(AffinityApplyOutcome {
                actual_affinity: affinity,
                affected_window_count: 1,
                applied_handle: target.key.handle,
            })
        }
    }

    fn target() -> AffinityTarget {
        WindowMetadata {
            key: WindowKey::new(WindowHandle::new(7), 11),
            root_handle: WindowHandle::new(7),
            title: "test".into(),
            class_name: "test".into(),
            process_name: "test".into(),
            root_class_name: "test".into(),
            root_process_name: "test".into(),
            rule_key: None,
            visible: true,
            cloaked: false,
            top_level: true,
            kind: WindowKind::Normal,
            hidden_from_list: false,
            icon: None,
        }
        .into()
    }

    fn receive_events(runtime: &WorkerRuntime, count: usize) -> Vec<WorkerEvent> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut events = Vec::with_capacity(count);
        while events.len() < count && Instant::now() < deadline {
            match runtime.try_recv() {
                Ok(Some(event)) => events.push(event),
                Ok(None) => thread::sleep(Duration::from_millis(2)),
                Err(error) => panic!("worker disconnected: {error}"),
            }
        }
        assert_eq!(events.len(), count, "timed out waiting for worker events");
        events
    }

    #[test]
    fn scan_and_affinity_run_off_the_calling_thread() {
        let platform = Arc::new(FakePlatform::default());
        let runtime = WorkerRuntime::spawn(platform.clone()).unwrap();
        let caller = thread::current().id();

        runtime.request_scan(4, ScanOptions::default()).unwrap();
        runtime
            .request_affinity(8, target(), Affinity::ExcludeFromCapture)
            .unwrap();
        let events = receive_events(&runtime, 2);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, WorkerEvent::Snapshot { generation: 4, .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, WorkerEvent::AffinityResult { request_id: 8, .. }))
        );

        let state = platform.state.lock();
        assert!(state.scan_threads.iter().all(|id| *id != caller));
        assert!(state.affinity_threads.iter().all(|id| *id != caller));
    }

    #[test]
    fn affinity_worker_preserves_fifo_order() {
        let platform = Arc::new(FakePlatform::default());
        let runtime = WorkerRuntime::spawn(platform.clone()).unwrap();
        runtime
            .request_affinity(1, target(), Affinity::ExcludeFromCapture)
            .unwrap();
        runtime
            .request_affinity(2, target(), Affinity::None)
            .unwrap();
        let events = receive_events(&runtime, 2);
        let request_ids: Vec<_> = events
            .into_iter()
            .filter_map(|event| match event {
                WorkerEvent::AffinityResult { request_id, .. } => Some(request_id),
                WorkerEvent::Snapshot { .. } => None,
            })
            .collect();
        assert_eq!(request_ids, vec![1, 2]);
        assert_eq!(
            platform.state.lock().applied,
            vec![Affinity::ExcludeFromCapture, Affinity::None]
        );
    }

    #[test]
    fn platform_errors_are_forwarded_with_generation() {
        let platform = Arc::new(FakePlatform::default());
        platform.state.lock().fail_scan = true;
        let runtime = WorkerRuntime::spawn(platform).unwrap();
        runtime.request_scan(99, ScanOptions::default()).unwrap();
        let event = receive_events(&runtime, 1).remove(0);
        match event {
            WorkerEvent::Snapshot { generation, result } => {
                assert_eq!(generation, 99);
                assert_eq!(result.unwrap_err().stage, OperationStage::EnumerateWindows);
            }
            WorkerEvent::AffinityResult { .. } => panic!("unexpected affinity event"),
        }
    }

    #[test]
    fn shutdown_rejects_new_work() {
        let platform = Arc::new(FakePlatform::default());
        let mut runtime = WorkerRuntime::spawn(platform).unwrap();
        runtime.shutdown();
        let error = runtime
            .request_scan(1, ScanOptions::default())
            .expect_err("shutdown runtime must reject commands");
        assert_eq!(error.stage, OperationStage::Shutdown);
    }
}
