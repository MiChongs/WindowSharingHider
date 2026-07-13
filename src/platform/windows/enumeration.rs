use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GA_ROOT, GetAncestor, GetClassNameW, GetWindowDisplayAffinity,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
};
use windows::core::{BOOL, PWSTR};

use crate::model::{
    Affinity, OperationError, OperationStage, WindowHandle, WindowKey, WindowMetadata,
    WindowSnapshot,
};
use crate::platform::{PlatformResult, ScanOptions};
use crate::policy::{
    WindowDescriptor, affinity_targets, classify_window, create_rule_key, should_include,
    should_scan_children,
};

use super::icons::icon_for_process_path;
use super::resources::OwnedHandle;

const MAX_CLASS_NAME: usize = 256;
const MAX_PROCESS_PATH: usize = 32_768;

#[derive(Clone, Debug, Default)]
struct ProcessIdentity {
    name: String,
    path: String,
}

struct EnumerationState {
    options: ScanOptions,
    windows: HashMap<WindowKey, WindowSnapshot>,
    child_roots: HashSet<WindowHandle>,
    callback_panicked: bool,
}

impl EnumerationState {
    fn visit_top_level(&mut self, hwnd: HWND) {
        let Some(snapshot) = build_window_snapshot(hwnd, None) else {
            return;
        };
        if should_include(
            &snapshot.metadata,
            self.options.include_system_candidates,
            self.options.include_wetype_candidate,
        ) {
            self.windows.insert(snapshot.metadata.key, snapshot.clone());
        }
        if should_scan_children(
            &snapshot.metadata,
            self.options.include_system_candidates,
            self.options.include_wetype_candidate,
        ) {
            self.child_roots.insert(snapshot.metadata.root_handle);
        }
    }

    fn visit_child(&mut self, hwnd: HWND, root: HWND) {
        let Some(snapshot) = build_window_snapshot(hwnd, Some(root)) else {
            return;
        };
        if should_include(
            &snapshot.metadata,
            self.options.include_system_candidates,
            self.options.include_wetype_candidate,
        ) {
            self.windows.insert(snapshot.metadata.key, snapshot);
        }
    }
}

struct ChildContext {
    state: *mut EnumerationState,
    root: HWND,
}

unsafe extern "system" fn top_level_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = unsafe { &mut *(lparam.0 as *mut EnumerationState) };
    match catch_unwind(AssertUnwindSafe(|| state.visit_top_level(hwnd))) {
        Ok(()) => BOOL(1),
        Err(_) => {
            state.callback_panicked = true;
            BOOL(0)
        }
    }
}

unsafe extern "system" fn child_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe { &mut *(lparam.0 as *mut ChildContext) };
    let state = unsafe { &mut *context.state };
    match catch_unwind(AssertUnwindSafe(|| state.visit_child(hwnd, context.root))) {
        Ok(()) => BOOL(1),
        Err(_) => {
            state.callback_panicked = true;
            BOOL(0)
        }
    }
}

pub(crate) fn enumerate_windows(options: ScanOptions) -> PlatformResult<Vec<WindowSnapshot>> {
    let mut state = EnumerationState {
        options,
        windows: HashMap::new(),
        child_roots: HashSet::new(),
        callback_panicked: false,
    };

    // SAFETY: lparam points to state for the complete synchronous enumeration.
    let top_result = unsafe {
        EnumWindows(
            Some(top_level_callback),
            LPARAM((&mut state as *mut EnumerationState) as isize),
        )
    };
    if state.callback_panicked {
        return Err(OperationError::new(
            OperationStage::EnumerateWindows,
            "窗口枚举回调发生 panic，已阻止其越过 FFI 边界",
        ));
    }
    if top_result.is_err() {
        return Err(OperationError::new(
            OperationStage::EnumerateWindows,
            "EnumWindows 调用失败",
        ));
    }

    if options.include_system_candidates || options.include_wetype_candidate {
        let roots: Vec<_> = state.child_roots.iter().copied().collect();
        for root in roots {
            let root_hwnd = HWND(root.raw() as *mut c_void);
            // SAFETY: root was produced by EnumWindows/GetAncestor. Invalidated
            // roots are skipped because windows may disappear between passes.
            if root.is_null() || !unsafe { IsWindow(Some(root_hwnd)).as_bool() } {
                continue;
            }
            let mut context = ChildContext {
                state: &mut state,
                root: root_hwnd,
            };
            // EnumChildWindows returns success when no child exists. Individual
            // metadata failures are intentionally skipped.
            let _ = unsafe {
                EnumChildWindows(
                    Some(root_hwnd),
                    Some(child_callback),
                    LPARAM((&mut context as *mut ChildContext) as isize),
                )
            };
            if state.callback_panicked {
                return Err(OperationError::new(
                    OperationStage::EnumerateWindows,
                    "子窗口枚举回调发生 panic，已阻止其越过 FFI 边界",
                ));
            }
        }
    }

    let mut snapshots: Vec<_> = state.windows.into_values().collect();
    snapshots.sort_by(|left, right| {
        left.metadata
            .display_title()
            .to_lowercase()
            .cmp(&right.metadata.display_title().to_lowercase())
            .then_with(|| {
                left.metadata
                    .key
                    .handle
                    .raw()
                    .cmp(&right.metadata.key.handle.raw())
            })
    });
    Ok(snapshots)
}

fn build_window_snapshot(hwnd: HWND, root_override: Option<HWND>) -> Option<WindowSnapshot> {
    // SAFETY: IsWindow accepts arbitrary HWND values and validates them.
    if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return None;
    }

    let root = root_override.unwrap_or_else(|| {
        // SAFETY: hwnd was validated directly above.
        unsafe { GetAncestor(hwnd, GA_ROOT) }
    });
    let root = if root.0.is_null() { hwnd } else { root };
    let top_level = root == hwnd;

    let title = window_text(hwnd);
    let class_name = window_class(hwnd);
    let process_id = window_process_id(hwnd);
    let owner_process = process_identity(process_id);
    let (root_class_name, root_process) = if top_level {
        (class_name.clone(), owner_process.clone())
    } else {
        let root_pid = window_process_id(root);
        (window_class(root), process_identity(root_pid))
    };
    let icon_path = if owner_process.path.is_empty() {
        &root_process.path
    } else {
        &owner_process.path
    };
    let icon = icon_for_process_path(icon_path);

    // SAFETY: hwnd remains best-effort valid; both APIs tolerate a window that
    // disappears and simply report false/failure.
    let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
    let mut cloaked = 0i32;
    // SAFETY: cloaked points to a correctly-sized i32 for DWMWA_CLOAKED.
    let cloaked_result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut i32).cast(),
            size_of::<i32>() as u32,
        )
    };
    let cloaked = cloaked_result.is_ok() && cloaked != 0;

    let kind = classify_window(WindowDescriptor {
        process_name: &owner_process.name,
        class_name: &class_name,
        title: &title,
        root_process_name: &root_process.name,
        root_class_name: &root_class_name,
        top_level,
    });
    let key = WindowKey::new(WindowHandle::new(hwnd.0 as isize), process_id);
    let mut metadata = WindowMetadata {
        key,
        root_handle: WindowHandle::new(root.0 as isize),
        title,
        class_name,
        process_name: owner_process.name,
        root_class_name,
        root_process_name: root_process.name,
        icon,
        rule_key: None,
        visible,
        cloaked,
        top_level,
        kind,
        hidden_from_list: kind == crate::model::WindowKind::WeTypeCandidate,
    };
    metadata.rule_key = create_rule_key(&metadata);

    let actual_affinity = affinity_targets(&metadata)
        .into_iter()
        .find_map(read_window_affinity)
        .unwrap_or(Affinity::None);
    Some(WindowSnapshot {
        metadata,
        actual_affinity,
    })
}

fn window_text(hwnd: HWND) -> String {
    // SAFETY: hwnd is best-effort valid; failure produces an empty title.
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    let mut buffer = vec![0u16; usize::try_from(length.max(0)).unwrap_or(0) + 1];
    // SAFETY: buffer is writable and includes the trailing null slot.
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(copied.max(0)).unwrap_or(0)])
}

fn window_class(hwnd: HWND) -> String {
    let mut buffer = [0u16; MAX_CLASS_NAME];
    // SAFETY: buffer is writable for MAX_CLASS_NAME UTF-16 units.
    let copied = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(copied.max(0)).unwrap_or(0)])
}

fn window_process_id(hwnd: HWND) -> u32 {
    let mut process_id = 0u32;
    // SAFETY: process_id points to writable storage; a disappearing window
    // yields zero and is handled as unknown metadata.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    }
    process_id
}

fn process_identity(process_id: u32) -> ProcessIdentity {
    if process_id == 0 {
        return ProcessIdentity::default();
    }
    // SAFETY: OpenProcess validates the PID; access is read-only and minimal.
    let Ok(raw_handle) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
    else {
        return ProcessIdentity::default();
    };
    let Ok(process) = OwnedHandle::from_raw(
        raw_handle,
        OperationStage::ResolveProcess,
        format!("无法读取 PID {process_id} 的进程名"),
    ) else {
        return ProcessIdentity::default();
    };

    let mut buffer = vec![0u16; MAX_PROCESS_PATH];
    let mut length = buffer.len() as u32;
    // SAFETY: process is valid and buffer/length describe writable UTF-16 storage.
    let result = unsafe {
        QueryFullProcessImageNameW(
            process.as_raw(),
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    if result.is_err() || length == 0 {
        return ProcessIdentity::default();
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    let name = Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned();
    ProcessIdentity { name, path }
}

pub(crate) fn read_window_affinity(handle: WindowHandle) -> Option<Affinity> {
    if handle.is_null() {
        return None;
    }
    let hwnd = HWND(handle.raw() as *mut c_void);
    let mut affinity = 0u32;
    // SAFETY: GetWindowDisplayAffinity validates HWND and writes one u32 value.
    if unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }.is_ok() {
        Some(Affinity::from_raw(affinity))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_enumeration_returns_unique_window_keys() {
        let snapshots = enumerate_windows(ScanOptions::default())
            .expect("enumerating the current Windows desktop should succeed");
        let unique: HashSet<_> = snapshots.iter().map(|item| item.metadata.key).collect();
        assert_eq!(unique.len(), snapshots.len());
        assert!(
            snapshots
                .iter()
                .all(|item| !item.metadata.key.handle.is_null())
        );
    }

    #[test]
    fn default_enumeration_only_returns_displayable_normal_windows() {
        let snapshots = enumerate_windows(ScanOptions::default()).unwrap();
        assert!(snapshots.iter().all(|item| {
            item.metadata.kind == crate::model::WindowKind::Normal
                && item.metadata.visible
                && !item.metadata.cloaked
                && !item.metadata.title.trim().is_empty()
        }));
    }
}
