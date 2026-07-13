use std::ffi::c_void;
use std::sync::Arc;

use parking_lot::Mutex;

use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_FAILED};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};

use crate::model::{OperationError, OperationStage};

#[derive(Debug)]
pub(crate) struct OwnedHandle {
    raw: HANDLE,
}

// A Win32 HANDLE is an opaque process-local value. Ownership is unique and the
// underlying kernel object is safe to close from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    pub(crate) fn from_raw(
        raw: HANDLE,
        stage: OperationStage,
        context: impl Into<String>,
    ) -> Result<Self, OperationError> {
        if raw.is_invalid() {
            return Err(last_error(stage, context));
        }
        Ok(Self { raw })
    }

    pub(crate) const fn as_raw(&self) -> HANDLE {
        self.raw
    }

    pub(crate) fn raw_value(&self) -> usize {
        self.raw.0 as usize
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.raw.is_invalid() {
            // SAFETY: OwnedHandle has unique ownership and invalidates the value
            // immediately after the one CloseHandle call.
            unsafe {
                let _ = CloseHandle(self.raw);
            }
            self.raw = HANDLE::default();
        }
    }
}

#[derive(Debug)]
pub(crate) struct RemoteAllocation {
    process: HANDLE,
    address: *mut c_void,
    size: usize,
}

// The allocation belongs to another process and is only accessed through Win32
// APIs. The owning process HANDLE remains alive in RemoteCallGuard.
unsafe impl Send for RemoteAllocation {}

impl RemoteAllocation {
    pub(crate) fn allocate(process: &OwnedHandle, size: usize) -> Result<Self, OperationError> {
        if size == 0 {
            return Err(OperationError::new(
                OperationStage::AllocateRemoteMemory,
                "远程代码不能为空",
            ));
        }

        // SAFETY: The process handle is valid and owned by the caller. A null
        // preferred address asks Windows to select a region in the target.
        let address = unsafe {
            VirtualAllocEx(
                process.as_raw(),
                None,
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        if address.is_null() {
            return Err(last_error(
                OperationStage::AllocateRemoteMemory,
                format!("无法分配 {size} 字节远程代码"),
            ));
        }

        Ok(Self {
            process: process.as_raw(),
            address,
            size,
        })
    }

    pub(crate) const fn address(&self) -> *mut c_void {
        self.address
    }

    pub(crate) const fn size(&self) -> usize {
        self.size
    }
}

impl Drop for RemoteAllocation {
    fn drop(&mut self) {
        if !self.address.is_null() && !self.process.is_invalid() {
            // SAFETY: This value uniquely owns the allocation. RemoteCallGuard
            // guarantees that the process handle outlives this field.
            unsafe {
                let _ = VirtualFreeEx(self.process, self.address, 0, MEM_RELEASE);
            }
            self.address = std::ptr::null_mut();
        }
    }
}

#[derive(Debug)]
pub(crate) struct RemoteCallGuard {
    // Field order is intentional: allocation and thread must be dropped before
    // the process handle they depend on.
    allocation: Option<RemoteAllocation>,
    thread: Option<OwnedHandle>,
    process: OwnedHandle,
}

unsafe impl Send for RemoteCallGuard {}

impl RemoteCallGuard {
    pub(crate) fn new(process: OwnedHandle, allocation: RemoteAllocation) -> Self {
        Self {
            allocation: Some(allocation),
            thread: None,
            process,
        }
    }

    pub(crate) fn process(&self) -> &OwnedHandle {
        &self.process
    }

    pub(crate) fn allocation(&self) -> &RemoteAllocation {
        self.allocation
            .as_ref()
            .expect("remote allocation exists until guard drop")
    }

    pub(crate) fn set_thread(&mut self, thread: OwnedHandle) {
        debug_assert!(self.thread.is_none());
        self.thread = Some(thread);
    }

    pub(crate) fn thread(&self) -> Option<&OwnedHandle> {
        self.thread.as_ref()
    }

    pub(crate) fn defer_cleanup(self) {
        let thread_value = match self.thread.as_ref() {
            Some(thread) => thread.raw_value(),
            None => return,
        };

        let shared = Arc::new(Mutex::new(Some(self)));
        let worker_state = Arc::clone(&shared);
        let spawn_result = std::thread::Builder::new()
            .name("remote-affinity-cleanup".into())
            .spawn(move || {
                let thread = HANDLE(thread_value as *mut c_void);
                // SAFETY: The guard stored in worker_state keeps the thread
                // HANDLE valid until this wait completes.
                unsafe {
                    let _ = WaitForSingleObject(thread, INFINITE);
                }
                let guard = worker_state.lock().take();
                drop(guard);
            });

        if spawn_result.is_err() {
            // Spawning a cleanup thread can only fail under severe resource
            // exhaustion. Leaking the tiny guard is safer than freeing code
            // while the target thread may still execute it. Windows reclaims
            // both handles and target memory when either process exits.
            let guard = shared.lock().take();
            if let Some(guard) = guard {
                std::mem::forget(guard);
            }
        }
    }
}

pub(crate) fn wait_failed(result: windows::Win32::Foundation::WAIT_EVENT) -> bool {
    result == WAIT_FAILED
}

pub(crate) fn last_error(stage: OperationStage, context: impl Into<String>) -> OperationError {
    // SAFETY: GetLastError reads thread-local state and has no preconditions.
    let code = unsafe { GetLastError().0 };
    OperationError::win32(stage, code, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_handle_without_ownership() {
        let error = OwnedHandle::from_raw(
            HANDLE::default(),
            OperationStage::OpenProcess,
            "invalid test handle",
        )
        .expect_err("null handles must be rejected");
        assert_eq!(error.stage, OperationStage::OpenProcess);
    }

    #[test]
    fn zero_sized_remote_allocation_is_rejected_before_ffi() {
        let pseudo = OwnedHandle {
            raw: HANDLE::default(),
        };
        let error = RemoteAllocation::allocate(&pseudo, 0)
            .expect_err("zero-sized allocation must fail deterministically");
        assert_eq!(error.stage, OperationStage::AllocateRemoteMemory);
        std::mem::forget(pseudo);
    }
}
