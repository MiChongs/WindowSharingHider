use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::model::{
    Affinity, AffinityApplyOutcome, ManagedWindow, OperationError, PendingOperation,
    ProtectionState, WindowIcon, WindowKey, WindowKind, WindowSnapshot,
};
use crate::platform::{AffinityTarget, ScanOptions, WindowPlatform};
use crate::policy::{migrate_system_policy, should_apply_dedicated_policy, should_auto_retry};
use crate::worker::{WorkerEvent, WorkerRuntime};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatusTone {
    #[default]
    Neutral,
    Active,
    Success,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowRowView {
    pub row_id: i32,
    pub title: String,
    pub detail: String,
    pub icon: Option<WindowIcon>,
    pub status: String,
    pub status_tone: StatusTone,
    pub enabled: bool,
    pub busy: bool,
    pub system_candidate: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppView {
    pub rows: Vec<WindowRowView>,
    pub status_message: String,
    pub status_tone: StatusTone,
    pub scanning: bool,
    pub system_mode: bool,
    pub wetype_enabled: bool,
    pub wetype_found: bool,
    pub protected_count: usize,
}

#[derive(Clone, Debug)]
struct AffinityIntent {
    request_id: u64,
    key: WindowKey,
    target: AffinityTarget,
    affinity: Affinity,
}

#[derive(Default)]
struct StateEffects {
    affinity: Vec<AffinityIntent>,
    scan_again: bool,
}

pub struct AppState {
    windows: HashMap<WindowKey, ManagedWindow>,
    row_keys: HashMap<i32, WindowKey>,
    row_order: Vec<WindowKey>,
    wetype_key: Option<WindowKey>,
    next_row_id: i32,
    next_request_id: u64,
    next_generation: u64,
    scan_in_flight: Option<u64>,
    refresh_queued: bool,
    system_mode: bool,
    wetype_enabled: bool,
    status_message: String,
    status_tone: StatusTone,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
            row_keys: HashMap::new(),
            row_order: Vec::new(),
            wetype_key: None,
            next_row_id: 1,
            next_request_id: 1,
            next_generation: 1,
            scan_in_flight: None,
            refresh_queued: false,
            system_mode: false,
            wetype_enabled: false,
            status_message: "正在准备窗口扫描…".into(),
            status_tone: StatusTone::Neutral,
        }
    }
}

impl AppState {
    fn scan_options(&self) -> ScanOptions {
        ScanOptions {
            include_system_candidates: self.system_mode,
            include_wetype_candidate: self.wetype_enabled,
        }
    }

    fn begin_refresh(&mut self) -> Option<(u64, ScanOptions)> {
        if self.scan_in_flight.is_some() {
            self.refresh_queued = true;
            return None;
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.scan_in_flight = Some(generation);
        self.status_message = "正在扫描桌面窗口…".into();
        self.status_tone = StatusTone::Active;
        Some((generation, self.scan_options()))
    }

    fn fail_scan_dispatch(&mut self, generation: u64, error: OperationError) {
        if self.scan_in_flight == Some(generation) {
            self.scan_in_flight = None;
        }
        self.status_message = format!("无法启动窗口扫描：{error}");
        self.status_tone = StatusTone::Error;
    }

    fn complete_snapshot(
        &mut self,
        generation: u64,
        result: Result<Vec<WindowSnapshot>, OperationError>,
    ) -> StateEffects {
        if self.scan_in_flight != Some(generation) {
            return StateEffects::default();
        }
        self.scan_in_flight = None;

        let mut effects = StateEffects {
            scan_again: std::mem::take(&mut self.refresh_queued),
            ..StateEffects::default()
        };
        let snapshots = match result {
            Ok(snapshots) => snapshots,
            Err(error) => {
                self.status_message = format!("窗口扫描失败：{error}");
                self.status_tone = StatusTone::Error;
                return effects;
            }
        };

        let policy_by_rule: HashMap<_, _> = self
            .windows
            .values()
            .filter_map(|window| {
                window.metadata.rule_key.as_ref().map(|rule| {
                    (
                        rule.clone(),
                        (window.policy_enabled, window.last_failed_target),
                    )
                })
            })
            .collect();
        let mut seen = HashSet::with_capacity(snapshots.len());
        let mut wetype_key = None;

        for snapshot in snapshots {
            let key = snapshot.metadata.key;
            seen.insert(key);
            let was_visible = self
                .windows
                .get(&key)
                .map(|window| window.metadata.visible)
                .unwrap_or(false);

            if let Some(window) = self.windows.get_mut(&key) {
                window.was_visible = was_visible;
                window.metadata = snapshot.metadata;
                window.actual_affinity = snapshot.actual_affinity;
                if window.metadata.kind == WindowKind::Normal && window.pending.is_none() {
                    window.policy_enabled = window.actual_affinity.is_capture_protected();
                }
            } else {
                let row_id = self.allocate_row_id();
                let mut window = ManagedWindow {
                    row_id,
                    policy_enabled: snapshot.actual_affinity.is_capture_protected(),
                    metadata: snapshot.metadata,
                    actual_affinity: snapshot.actual_affinity,
                    pending: None,
                    last_failed_target: None,
                    was_visible: false,
                };
                if let Some(rule) = window.metadata.rule_key.as_ref()
                    && let Some((enabled, failed)) = policy_by_rule.get(rule)
                {
                    let source = ManagedWindow {
                        row_id: 0,
                        metadata: window.metadata.clone(),
                        actual_affinity: Affinity::None,
                        policy_enabled: *enabled,
                        pending: None,
                        last_failed_target: *failed,
                        was_visible: false,
                    };
                    migrate_system_policy(&source, &mut window);
                }
                self.windows.insert(key, window);
            }

            let window = self
                .windows
                .get_mut(&key)
                .expect("window was inserted or updated above");
            if window.metadata.kind == WindowKind::WeTypeCandidate {
                wetype_key = Some(key);
                window.policy_enabled = self.wetype_enabled;
                if should_apply_dedicated_policy(window, was_visible, self.wetype_enabled) {
                    effects.affinity.push(Self::begin_affinity(
                        &mut self.next_request_id,
                        window,
                        Affinity::requested(self.wetype_enabled),
                        true,
                    ));
                }
            } else if should_auto_retry(window, was_visible) {
                effects.affinity.push(Self::begin_affinity(
                    &mut self.next_request_id,
                    window,
                    Affinity::ExcludeFromCapture,
                    true,
                ));
            }
        }

        self.windows.retain(|key, _| seen.contains(key));
        self.wetype_key = wetype_key.filter(|key| self.windows.contains_key(key));
        self.rebuild_row_indexes();
        self.status_message = format!("已发现 {} 个可选择窗口", self.row_order.len());
        self.status_tone = StatusTone::Neutral;
        effects
    }

    fn allocate_row_id(&mut self) -> i32 {
        let row_id = self.next_row_id;
        self.next_row_id = self.next_row_id.checked_add(1).unwrap_or(1);
        row_id
    }

    fn rebuild_row_indexes(&mut self) {
        self.row_order = self
            .windows
            .iter()
            .filter_map(|(key, window)| (!window.metadata.hidden_from_list).then_some(*key))
            .collect();
        self.row_order.sort_by(|left, right| {
            let left_window = &self.windows[left];
            let right_window = &self.windows[right];
            left_window
                .metadata
                .display_title()
                .to_lowercase()
                .cmp(&right_window.metadata.display_title().to_lowercase())
                .then_with(|| left.handle.raw().cmp(&right.handle.raw()))
        });
        self.row_keys.clear();
        self.row_keys.extend(self.row_order.iter().map(|key| {
            let window = &self.windows[key];
            (window.row_id, *key)
        }));
    }

    fn begin_affinity(
        next_request_id: &mut u64,
        window: &mut ManagedWindow,
        affinity: Affinity,
        automatic: bool,
    ) -> AffinityIntent {
        let request_id = *next_request_id;
        *next_request_id = next_request_id.wrapping_add(1).max(1);
        window.pending = Some(PendingOperation {
            request_id,
            target: affinity,
            automatic,
        });
        window.last_failed_target = None;
        if window.metadata.kind.is_system_candidate() {
            window.policy_enabled = affinity.is_capture_protected();
        }
        AffinityIntent {
            request_id,
            key: window.metadata.key,
            target: window.metadata.clone().into(),
            affinity,
        }
    }

    fn toggle_row(&mut self, row_id: i32, enabled: bool) -> Option<AffinityIntent> {
        let key = *self.row_keys.get(&row_id)?;
        let window = self.windows.get_mut(&key)?;
        let intent = Self::begin_affinity(
            &mut self.next_request_id,
            window,
            Affinity::requested(enabled),
            false,
        );
        self.status_message = if enabled {
            format!("正在保护“{}”…", window.metadata.display_title())
        } else {
            format!("正在取消“{}”的保护…", window.metadata.display_title())
        };
        self.status_tone = StatusTone::Active;
        Some(intent)
    }

    fn set_system_mode(&mut self, enabled: bool) {
        self.system_mode = enabled;
        self.status_message = if enabled {
            "已启用高级系统/输入法窗口扫描".into()
        } else {
            "已关闭高级系统/输入法窗口扫描".into()
        };
        self.status_tone = StatusTone::Neutral;
    }

    fn set_wetype_enabled(&mut self, enabled: bool) -> Option<AffinityIntent> {
        self.wetype_enabled = enabled;
        let Some(key) = self.wetype_key else {
            self.status_message = if enabled {
                "正在等待微信输入法候选框出现…".into()
            } else {
                "微信输入法候选框保护已关闭".into()
            };
            self.status_tone = if enabled {
                StatusTone::Active
            } else {
                StatusTone::Neutral
            };
            return None;
        };
        let window = self.windows.get_mut(&key)?;
        window.policy_enabled = enabled;
        let intent = Self::begin_affinity(
            &mut self.next_request_id,
            window,
            Affinity::requested(enabled),
            false,
        );
        self.status_message = if enabled {
            "正在保护微信输入法候选框…".into()
        } else {
            "正在取消微信输入法候选框保护…".into()
        };
        self.status_tone = StatusTone::Active;
        Some(intent)
    }

    fn complete_affinity(
        &mut self,
        request_id: u64,
        key: WindowKey,
        requested: Affinity,
        result: Result<AffinityApplyOutcome, OperationError>,
    ) -> bool {
        let Some(window) = self.windows.get_mut(&key) else {
            return false;
        };
        if window.pending.map(|pending| pending.request_id) != Some(request_id) {
            return false;
        }
        window.pending = None;
        match result {
            Ok(outcome) => {
                window.actual_affinity = outcome.actual_affinity;
                window.last_failed_target = None;
                if window.metadata.kind.is_system_candidate() {
                    window.policy_enabled = requested.is_capture_protected();
                }
                self.status_message = if requested.is_capture_protected() {
                    format!("“{}”已从支持的捕获中排除", window.metadata.display_title())
                } else {
                    format!("“{}”已恢复到捕获画面", window.metadata.display_title())
                };
                self.status_tone = StatusTone::Success;
            }
            Err(error) => {
                window.last_failed_target = Some(requested);
                if window.metadata.kind.is_system_candidate() {
                    window.policy_enabled = window.actual_affinity.is_capture_protected();
                }
                self.status_message =
                    format!("无法更新“{}”：{error}", window.metadata.display_title());
                self.status_tone = StatusTone::Error;
            }
        }
        true
    }

    pub fn view(&self) -> AppView {
        let rows = self
            .row_order
            .iter()
            .filter_map(|key| self.windows.get(key))
            .map(|window| {
                let state = window.protection_state();
                WindowRowView {
                    row_id: window.row_id,
                    title: window.metadata.display_title().into(),
                    detail: window.metadata.technical_identity(),
                    icon: window.metadata.icon.clone(),
                    status: state.label().into(),
                    status_tone: match state {
                        ProtectionState::Protected => StatusTone::Success,
                        ProtectionState::ApplyingProtection
                        | ProtectionState::RemovingProtection => StatusTone::Active,
                        ProtectionState::FailedToProtect | ProtectionState::FailedToRemove => {
                            StatusTone::Error
                        }
                        ProtectionState::Unprotected => StatusTone::Neutral,
                    },
                    enabled: window.displayed_enabled(),
                    busy: window.pending.is_some(),
                    system_candidate: window.metadata.kind.is_system_candidate(),
                }
            })
            .collect::<Vec<_>>();
        AppView {
            protected_count: rows.iter().filter(|row| row.enabled).count(),
            rows,
            status_message: self.status_message.clone(),
            status_tone: self.status_tone,
            scanning: self.scan_in_flight.is_some(),
            system_mode: self.system_mode,
            wetype_enabled: self.wetype_enabled,
            wetype_found: self.wetype_key.is_some(),
        }
    }
}

pub struct AppController {
    state: AppState,
    runtime: WorkerRuntime,
}

impl AppController {
    pub fn new(platform: Arc<dyn WindowPlatform>) -> Result<Self, OperationError> {
        Ok(Self {
            state: AppState::default(),
            runtime: WorkerRuntime::spawn(platform)?,
        })
    }

    pub fn request_refresh(&mut self) {
        let Some((generation, options)) = self.state.begin_refresh() else {
            return;
        };
        if let Err(error) = self.runtime.request_scan(generation, options) {
            self.state.fail_scan_dispatch(generation, error);
        }
    }

    pub fn toggle_window(&mut self, row_id: i32, enabled: bool) {
        if let Some(intent) = self.state.toggle_row(row_id, enabled) {
            self.dispatch_affinity(intent);
        }
    }

    pub fn set_system_mode(&mut self, enabled: bool) {
        self.state.set_system_mode(enabled);
        self.request_refresh();
    }

    pub fn set_wetype_enabled(&mut self, enabled: bool) {
        if let Some(intent) = self.state.set_wetype_enabled(enabled) {
            self.dispatch_affinity(intent);
        }
        self.request_refresh();
    }

    pub fn drain_worker_events(&mut self) -> bool {
        let mut changed = false;
        loop {
            let event = match self.runtime.try_recv() {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(error) => {
                    self.state.status_message = format!("后台任务连接已关闭：{error}");
                    self.state.status_tone = StatusTone::Error;
                    changed = true;
                    break;
                }
            };
            changed = true;
            match event {
                WorkerEvent::Snapshot { generation, result } => {
                    let effects = self.state.complete_snapshot(generation, result);
                    for intent in effects.affinity {
                        self.dispatch_affinity(intent);
                    }
                    if effects.scan_again {
                        self.request_refresh();
                    }
                }
                WorkerEvent::AffinityResult {
                    request_id,
                    key,
                    requested,
                    result,
                } => {
                    if self
                        .state
                        .complete_affinity(request_id, key, requested, result)
                    {
                        self.request_refresh();
                    }
                }
            }
        }
        changed
    }

    pub fn view(&self) -> AppView {
        self.state.view()
    }

    pub fn shutdown(&mut self) {
        self.runtime.shutdown();
    }

    fn dispatch_affinity(&mut self, intent: AffinityIntent) {
        if let Err(error) =
            self.runtime
                .request_affinity(intent.request_id, intent.target, intent.affinity)
        {
            let _ = self.state.complete_affinity(
                intent.request_id,
                intent.key,
                intent.affinity,
                Err(error),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{OperationStage, WindowHandle, WindowMetadata};

    fn snapshot(handle: isize, process_id: u32, title: &str, kind: WindowKind) -> WindowSnapshot {
        let mut metadata = WindowMetadata {
            key: WindowKey::new(WindowHandle::new(handle), process_id),
            root_handle: WindowHandle::new(handle),
            title: title.into(),
            class_name: format!("{title}Class"),
            process_name: format!("{title}Process"),
            root_class_name: format!("{title}Class"),
            root_process_name: format!("{title}Process"),
            rule_key: None,
            visible: true,
            cloaked: false,
            top_level: true,
            kind,
            hidden_from_list: kind == WindowKind::WeTypeCandidate,
            icon: None,
        };
        if kind.is_system_candidate() {
            metadata.rule_key = Some(format!("{}|{}", metadata.process_name, metadata.class_name));
        }
        WindowSnapshot {
            metadata,
            actual_affinity: Affinity::None,
        }
    }

    fn complete(state: &mut AppState, snapshots: Vec<WindowSnapshot>) -> StateEffects {
        let (generation, _) = state.begin_refresh().unwrap();
        state.complete_snapshot(generation, Ok(snapshots))
    }

    #[test]
    fn refresh_requests_are_coalesced_without_parallel_generations() {
        let mut state = AppState::default();
        let (generation, _) = state.begin_refresh().unwrap();
        assert!(state.begin_refresh().is_none());
        let effects = state.complete_snapshot(generation, Ok(Vec::new()));
        assert!(effects.scan_again);
        assert!(state.scan_in_flight.is_none());
    }

    #[test]
    fn snapshot_diff_keeps_rows_stable_and_removes_missing_windows() {
        let mut state = AppState::default();
        complete(
            &mut state,
            vec![
                snapshot(1, 10, "Beta", WindowKind::Normal),
                snapshot(2, 10, "Alpha", WindowKind::Normal),
            ],
        );
        let first = state.view();
        assert_eq!(
            first
                .rows
                .iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Beta"]
        );
        let alpha_id = first.rows[0].row_id;

        complete(
            &mut state,
            vec![snapshot(2, 10, "Alpha renamed", WindowKind::Normal)],
        );
        let second = state.view();
        assert_eq!(second.rows.len(), 1);
        assert_eq!(second.rows[0].row_id, alpha_id);
        assert_eq!(second.rows[0].title, "Alpha renamed");
    }

    #[test]
    fn stale_scan_generation_is_ignored() {
        let mut state = AppState::default();
        let (generation, _) = state.begin_refresh().unwrap();
        let effects = state.complete_snapshot(
            generation + 1,
            Ok(vec![snapshot(1, 1, "stale", WindowKind::Normal)]),
        );
        assert!(effects.affinity.is_empty());
        assert!(state.view().rows.is_empty());
        assert_eq!(state.scan_in_flight, Some(generation));
    }

    #[test]
    fn system_policy_migrates_across_recreated_handle() {
        let mut state = AppState::default();
        let first_snapshot = snapshot(10, 7, "IME", WindowKind::InputMethod);
        let rule = first_snapshot.metadata.rule_key.clone();
        complete(&mut state, vec![first_snapshot]);
        let row_id = state.view().rows[0].row_id;
        let intent = state.toggle_row(row_id, true).unwrap();
        state.complete_affinity(
            intent.request_id,
            intent.key,
            Affinity::ExcludeFromCapture,
            Ok(AffinityApplyOutcome {
                actual_affinity: Affinity::ExcludeFromCapture,
                affected_window_count: 1,
                applied_handle: intent.key.handle,
            }),
        );

        let mut recreated = snapshot(11, 7, "IME new", WindowKind::InputMethod);
        recreated.metadata.rule_key = rule;
        let effects = complete(&mut state, vec![recreated]);
        assert!(state.view().rows[0].enabled);
        assert_eq!(
            effects.affinity.len(),
            1,
            "newly visible recreated system window should auto-retry"
        );
    }

    #[test]
    fn stale_affinity_result_cannot_override_latest_request() {
        let mut state = AppState::default();
        complete(&mut state, vec![snapshot(20, 4, "App", WindowKind::Normal)]);
        let row_id = state.view().rows[0].row_id;
        let first = state.toggle_row(row_id, true).unwrap();
        let second = state.toggle_row(row_id, false).unwrap();
        assert!(!state.complete_affinity(
            first.request_id,
            first.key,
            first.affinity,
            Ok(AffinityApplyOutcome {
                actual_affinity: Affinity::ExcludeFromCapture,
                affected_window_count: 1,
                applied_handle: first.key.handle,
            }),
        ));
        assert_eq!(
            state.windows[&second.key].pending.unwrap().request_id,
            second.request_id
        );
        assert!(!state.view().rows[0].enabled);
    }

    #[test]
    fn wetype_policy_waits_then_applies_when_candidate_appears() {
        let mut state = AppState::default();
        assert!(state.set_wetype_enabled(true).is_none());
        let effects = complete(
            &mut state,
            vec![snapshot(
                30,
                9,
                "wetype_candidate",
                WindowKind::WeTypeCandidate,
            )],
        );
        assert_eq!(effects.affinity.len(), 1);
        assert!(state.view().wetype_found);
        assert!(
            state.view().rows.is_empty(),
            "dedicated candidate stays out of generic list"
        );
    }

    #[test]
    fn scan_error_is_visible_without_erasing_existing_rows() {
        let mut state = AppState::default();
        complete(&mut state, vec![snapshot(1, 1, "App", WindowKind::Normal)]);
        let (generation, _) = state.begin_refresh().unwrap();
        state.complete_snapshot(
            generation,
            Err(OperationError::new(
                OperationStage::EnumerateWindows,
                "boom",
            )),
        );
        let view = state.view();
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.status_tone, StatusTone::Error);
        assert!(view.status_message.contains("boom"));
    }
}
