use std::ffi::c_void;
use std::mem::{size_of, transmute};

use windows::Win32::Foundation::{HMODULE, HWND, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Diagnostics::Debug::{FlushInstructionCache, WriteProcessMemory};
use windows::Win32::System::ProcessStatus::{
    K32EnumProcessModulesEx, K32GetModuleFileNameExW, K32GetModuleInformation, LIST_MODULES_ALL,
    MODULEINFO,
};
use windows::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_CREATE_THREAD,
    PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
    WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};

use crate::model::{Affinity, AffinityApplyOutcome, OperationError, OperationStage, WindowHandle};
use crate::platform::{AffinityTarget, PlatformResult};
use crate::policy::affinity_targets;

use super::enumeration::read_window_affinity;
use super::remote::{
    ExportResolver, ProcessMemoryReader, RemoteModule, TargetArchitecture, build_call_stub,
};
use super::resources::{OwnedHandle, RemoteAllocation, RemoteCallGuard, last_error, wait_failed};

const REMOTE_THREAD_TIMEOUT_MS: u32 = 10_000;
const MAX_MODULES: usize = 16_384;
const MAX_MODULE_PATH: usize = 32_768;

pub(crate) fn apply_window_affinity(
    target: AffinityTarget,
    affinity: Affinity,
) -> PlatformResult<AffinityApplyOutcome> {
    if !matches!(affinity, Affinity::None | Affinity::ExcludeFromCapture) {
        return Err(OperationError::new(
            OperationStage::ValidateWindow,
            format!("不允许写入 affinity 0x{:X}", affinity.raw()),
        ));
    }

    let handles = affinity_targets(&target.metadata);
    if handles.is_empty() {
        return Err(OperationError::new(
            OperationStage::ValidateWindow,
            "没有可用的捕获排除目标窗口",
        ));
    }

    let mut failures = Vec::new();
    let mut last_error_value = None;
    for handle in handles {
        match apply_single_window(handle, affinity) {
            Ok(actual_affinity) => {
                return Ok(AffinityApplyOutcome {
                    actual_affinity,
                    affected_window_count: 1,
                    applied_handle: handle,
                });
            }
            Err(error) => {
                failures.push(format!("窗口 {handle}：{error}"));
                last_error_value = Some(error);
            }
        }
    }

    let mut error = last_error_value.unwrap_or_else(|| {
        OperationError::new(OperationStage::ValidateWindow, "没有执行任何 affinity 调用")
    });
    error.context = failures.join("；");
    Err(error)
}

fn apply_single_window(handle: WindowHandle, affinity: Affinity) -> PlatformResult<Affinity> {
    if handle.is_null() {
        return Err(OperationError::new(
            OperationStage::ValidateWindow,
            "目标 HWND 为空",
        ));
    }
    let hwnd = HWND(handle.raw() as *mut c_void);
    // SAFETY: IsWindow validates an arbitrary HWND value.
    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return Err(OperationError::new(
            OperationStage::ValidateWindow,
            format!("窗口 {handle} 已关闭或已重新创建"),
        ));
    }

    let mut process_id = 0u32;
    // SAFETY: process_id points to writable storage.
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    if thread_id == 0 || process_id == 0 {
        return Err(last_error(
            OperationStage::ResolveProcess,
            format!("无法解析窗口 {handle} 的所属进程"),
        ));
    }

    let access = PROCESS_CREATE_THREAD
        | PROCESS_QUERY_INFORMATION
        | PROCESS_VM_OPERATION
        | PROCESS_VM_WRITE
        | PROCESS_VM_READ;
    // SAFETY: OpenProcess validates PID/access; no handle inheritance is requested.
    let raw_process = unsafe { OpenProcess(access, false, process_id) }.map_err(|_| {
        last_error(
            OperationStage::OpenProcess,
            format!("无法以最小所需权限打开 PID {process_id}"),
        )
    })?;
    let process = OwnedHandle::from_raw(
        raw_process,
        OperationStage::OpenProcess,
        format!("OpenProcess 返回无效 PID {process_id} 句柄"),
    )?;

    let architecture = TargetArchitecture::detect(&process)?;
    let modules = enumerate_remote_modules(&process)?;
    let reader = ProcessMemoryReader::new(&process);
    let function_address =
        ExportResolver::new(&reader, &modules).resolve("user32.dll", "SetWindowDisplayAffinity")?;
    let stub = build_call_stub(handle, affinity.raw(), function_address, architecture)?;

    let allocation = RemoteAllocation::allocate(&process, stub.len())?;
    let mut guard = RemoteCallGuard::new(process, allocation);
    let mut bytes_written = 0usize;
    // SAFETY: The remote allocation is valid for stub.len() bytes and the local
    // source slice is readable for the same length.
    let write_result = unsafe {
        WriteProcessMemory(
            guard.process().as_raw(),
            guard.allocation().address(),
            stub.as_ptr().cast(),
            stub.len(),
            Some(&mut bytes_written),
        )
    };
    if write_result.is_err() || bytes_written != stub.len() {
        return Err(last_error(
            OperationStage::WriteRemoteMemory,
            format!(
                "需要写入 {} 字节远程代码，实际写入 {bytes_written} 字节",
                stub.len()
            ),
        ));
    }

    // SAFETY: The address/length describe the code written immediately above.
    if unsafe {
        FlushInstructionCache(
            guard.process().as_raw(),
            Some(guard.allocation().address()),
            guard.allocation().size(),
        )
    }
    .is_err()
    {
        return Err(last_error(
            OperationStage::FlushInstructionCache,
            "无法刷新目标进程的远程代码缓存",
        ));
    }

    type RemoteStart = unsafe extern "system" fn(*mut c_void) -> u32;
    // SAFETY: allocation.address points to the architecture-specific stub built
    // above. CreateRemoteThread requires the same ABI function pointer shape.
    let start_routine: RemoteStart = unsafe { transmute(guard.allocation().address()) };
    let raw_thread = unsafe {
        CreateRemoteThread(
            guard.process().as_raw(),
            None,
            0,
            Some(start_routine),
            None,
            0,
            None,
        )
    }
    .map_err(|_| {
        last_error(
            OperationStage::CreateRemoteThread,
            "CreateRemoteThread 无法启动 SetWindowDisplayAffinity stub",
        )
    })?;
    let thread = OwnedHandle::from_raw(
        raw_thread,
        OperationStage::CreateRemoteThread,
        "CreateRemoteThread 返回无效句柄",
    )?;
    guard.set_thread(thread);

    // SAFETY: guard owns a valid thread HANDLE for this wait.
    let wait_result = unsafe {
        WaitForSingleObject(
            guard.thread().expect("thread is set before wait").as_raw(),
            REMOTE_THREAD_TIMEOUT_MS,
        )
    };
    if wait_result == WAIT_TIMEOUT {
        guard.defer_cleanup();
        return Err(OperationError::new(
            OperationStage::WaitForRemoteThread,
            "远程调用等待 10 秒后超时；资源将在目标线程结束后清理",
        ));
    }
    if wait_failed(wait_result) || wait_result == WAIT_FAILED {
        guard.defer_cleanup();
        return Err(last_error(
            OperationStage::WaitForRemoteThread,
            "等待远程线程失败；资源已移交安全清理线程",
        ));
    }
    if wait_result != WAIT_OBJECT_0 {
        guard.defer_cleanup();
        return Err(OperationError::new(
            OperationStage::WaitForRemoteThread,
            format!("远程线程返回未知等待状态 0x{:08X}", wait_result.0),
        ));
    }

    let mut exit_code = 0u32;
    // SAFETY: WAIT_OBJECT_0 proves the owned thread has terminated.
    if unsafe {
        GetExitCodeThread(
            guard
                .thread()
                .expect("thread remains set until guard drop")
                .as_raw(),
            &mut exit_code,
        )
    }
    .is_err()
    {
        return Err(last_error(
            OperationStage::ReadRemoteResult,
            "无法读取远程 SetWindowDisplayAffinity 返回值",
        ));
    }
    if exit_code == 0 {
        return Err(OperationError::new(
            OperationStage::ReadRemoteResult,
            "目标进程中的 SetWindowDisplayAffinity 返回 FALSE",
        ));
    }

    drop(guard);
    read_window_affinity(handle).ok_or_else(|| {
        last_error(
            OperationStage::ReadAffinity,
            "远程调用成功，但无法读取实际 display affinity",
        )
    })
}

fn enumerate_remote_modules(process: &OwnedHandle) -> PlatformResult<Vec<RemoteModule>> {
    let mut bytes_needed = 0u32;
    // SAFETY: First call intentionally supplies no buffer to obtain required size.
    let probe_result = unsafe {
        K32EnumProcessModulesEx(
            process.as_raw(),
            std::ptr::null_mut(),
            0,
            &mut bytes_needed,
            LIST_MODULES_ALL.0 as u32,
        )
    };
    if !probe_result.as_bool() || bytes_needed == 0 {
        return Err(last_error(
            OperationStage::EnumerateModules,
            "无法探测目标进程模块数量",
        ));
    }

    let module_count = bytes_needed as usize / size_of::<HMODULE>();
    if module_count == 0 || module_count > MAX_MODULES {
        return Err(OperationError::new(
            OperationStage::EnumerateModules,
            format!("目标模块数量 {module_count} 无效或超过安全上限"),
        ));
    }
    let mut handles = vec![HMODULE::default(); module_count];
    // SAFETY: handles has bytes_needed capacity as derived above.
    let enumerate_result = unsafe {
        K32EnumProcessModulesEx(
            process.as_raw(),
            handles.as_mut_ptr(),
            bytes_needed,
            &mut bytes_needed,
            LIST_MODULES_ALL.0 as u32,
        )
    };
    if !enumerate_result.as_bool() {
        return Err(last_error(
            OperationStage::EnumerateModules,
            "无法读取目标进程模块句柄",
        ));
    }

    let returned_count = (bytes_needed as usize / size_of::<HMODULE>()).min(handles.len());
    let mut modules = Vec::with_capacity(returned_count);
    for module in handles.into_iter().take(returned_count) {
        let mut info = MODULEINFO::default();
        // SAFETY: info points to a correctly-sized MODULEINFO.
        if !unsafe {
            K32GetModuleInformation(
                process.as_raw(),
                module,
                &mut info,
                size_of::<MODULEINFO>() as u32,
            )
        }
        .as_bool()
        {
            continue;
        }

        let mut path = vec![0u16; MAX_MODULE_PATH];
        // SAFETY: path is writable and module came from the same process.
        let copied =
            unsafe { K32GetModuleFileNameExW(Some(process.as_raw()), Some(module), &mut path) };
        if copied == 0 {
            continue;
        }
        let full_path = String::from_utf16_lossy(&path[..copied as usize]);
        let name = std::path::Path::new(&full_path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&full_path)
            .to_owned();
        modules.push(RemoteModule {
            name,
            base: info.lpBaseOfDll as u64,
            size: info.SizeOfImage,
        });
    }

    if modules.is_empty() {
        return Err(OperationError::new(
            OperationStage::EnumerateModules,
            "目标进程没有可读取的模块",
        ));
    }
    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{WindowKey, WindowKind, WindowMetadata};

    fn invalid_target() -> AffinityTarget {
        let metadata = WindowMetadata {
            key: WindowKey::new(WindowHandle::NULL, 0),
            root_handle: WindowHandle::NULL,
            title: "invalid".into(),
            class_name: "invalid".into(),
            process_name: "invalid".into(),
            root_class_name: "invalid".into(),
            root_process_name: "invalid".into(),
            rule_key: None,
            visible: false,
            cloaked: false,
            top_level: true,
            kind: WindowKind::Normal,
            hidden_from_list: false,
            icon: None,
        };
        metadata.into()
    }

    #[test]
    fn rejects_invalid_window_before_opening_process() {
        let error = apply_window_affinity(invalid_target(), Affinity::ExcludeFromCapture)
            .expect_err("null HWND must be rejected");
        assert_eq!(error.stage, OperationStage::ValidateWindow);
    }

    #[test]
    fn rejects_affinity_values_the_app_never_writes() {
        let error = apply_window_affinity(invalid_target(), Affinity::Monitor)
            .expect_err("WDA_MONITOR is read-only for this app");
        assert_eq!(error.stage, OperationStage::ValidateWindow);
    }
}
