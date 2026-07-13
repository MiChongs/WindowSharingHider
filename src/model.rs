use std::fmt;
use std::sync::Arc;

pub const WDA_NONE: u32 = 0x0000_0000;
pub const WDA_MONITOR: u32 = 0x0000_0001;
pub const WDA_EXCLUDE_FROM_CAPTURE: u32 = 0x0000_0011;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct WindowHandle(isize);

impl WindowHandle {
    pub const NULL: Self = Self(0);

    pub const fn new(raw: isize) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> isize {
        self.0
    }

    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for WindowHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "0x{:X}", self.0 as usize)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WindowKey {
    pub handle: WindowHandle,
    pub process_id: u32,
}

impl WindowKey {
    pub const fn new(handle: WindowHandle, process_id: u32) -> Self {
        Self { handle, process_id }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowKind {
    Normal,
    System,
    InputMethod,
    WeTypeCandidate,
}

impl WindowKind {
    pub const fn is_system_candidate(self) -> bool {
        matches!(
            self,
            Self::System | Self::InputMethod | Self::WeTypeCandidate
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Affinity {
    #[default]
    None,
    Monitor,
    ExcludeFromCapture,
    Other(u32),
}

impl Affinity {
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            WDA_NONE => Self::None,
            WDA_MONITOR => Self::Monitor,
            WDA_EXCLUDE_FROM_CAPTURE => Self::ExcludeFromCapture,
            other => Self::Other(other),
        }
    }

    pub const fn raw(self) -> u32 {
        match self {
            Self::None => WDA_NONE,
            Self::Monitor => WDA_MONITOR,
            Self::ExcludeFromCapture => WDA_EXCLUDE_FROM_CAPTURE,
            Self::Other(raw) => raw,
        }
    }

    pub const fn is_capture_protected(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn requested(enabled: bool) -> Self {
        if enabled {
            Self::ExcludeFromCapture
        } else {
            Self::None
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowIcon {
    source: Arc<str>,
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
}

impl WindowIcon {
    pub fn from_rgba(
        source: impl Into<Arc<str>>,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Option<Self> {
        let expected_len = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(4)?;
        if width == 0 || height == 0 || rgba.len() != expected_len {
            return None;
        }
        Some(Self {
            source: source.into(),
            width,
            height,
            rgba: rgba.into(),
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowMetadata {
    pub key: WindowKey,
    pub root_handle: WindowHandle,
    pub title: String,
    pub class_name: String,
    pub process_name: String,
    pub root_class_name: String,
    pub root_process_name: String,
    pub icon: Option<WindowIcon>,
    pub rule_key: Option<String>,
    pub visible: bool,
    pub cloaked: bool,
    pub top_level: bool,
    pub kind: WindowKind,
    pub hidden_from_list: bool,
}

impl WindowMetadata {
    pub fn display_title(&self) -> &str {
        if self.kind == WindowKind::WeTypeCandidate {
            "微信输入法候选框"
        } else if self.title == "Program Manager" {
            "桌面和图标"
        } else if self.title.trim().is_empty() {
            "（无标题）"
        } else {
            &self.title
        }
    }

    pub fn technical_identity(&self) -> String {
        let process = if self.process_name.is_empty() {
            "unknown-process"
        } else {
            &self.process_name
        };
        let class_name = if self.class_name.is_empty() {
            "unknown-class"
        } else {
            &self.class_name
        };
        format!("{process} · {class_name} · {}", self.key.handle)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowSnapshot {
    pub metadata: WindowMetadata,
    pub actual_affinity: Affinity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingOperation {
    pub request_id: u64,
    pub target: Affinity,
    pub automatic: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectionState {
    Unprotected,
    Protected,
    ApplyingProtection,
    RemovingProtection,
    FailedToProtect,
    FailedToRemove,
}

impl ProtectionState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unprotected => "未保护",
            Self::Protected => "已保护",
            Self::ApplyingProtection => "正在启用",
            Self::RemovingProtection => "正在停用",
            Self::FailedToProtect => "启用失败",
            Self::FailedToRemove => "停用失败",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedWindow {
    pub row_id: i32,
    pub metadata: WindowMetadata,
    pub actual_affinity: Affinity,
    pub policy_enabled: bool,
    pub pending: Option<PendingOperation>,
    pub last_failed_target: Option<Affinity>,
    pub was_visible: bool,
}

impl ManagedWindow {
    pub fn displayed_enabled(&self) -> bool {
        self.pending
            .map(|operation| operation.target.is_capture_protected())
            .unwrap_or_else(|| {
                if self.metadata.kind.is_system_candidate() {
                    self.policy_enabled
                } else {
                    self.actual_affinity.is_capture_protected()
                }
            })
    }

    pub fn protection_state(&self) -> ProtectionState {
        if let Some(operation) = self.pending {
            return if operation.target.is_capture_protected() {
                ProtectionState::ApplyingProtection
            } else {
                ProtectionState::RemovingProtection
            };
        }

        if let Some(target) = self.last_failed_target {
            return if target.is_capture_protected() {
                ProtectionState::FailedToProtect
            } else {
                ProtectionState::FailedToRemove
            };
        }

        if self.actual_affinity.is_capture_protected() {
            ProtectionState::Protected
        } else {
            ProtectionState::Unprotected
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffinityApplyOutcome {
    pub actual_affinity: Affinity,
    pub affected_window_count: u32,
    pub applied_handle: WindowHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationStage {
    ValidateWindow,
    ResolveProcess,
    OpenProcess,
    DetectArchitecture,
    EnumerateModules,
    ReadRemoteImage,
    ResolveExport,
    AllocateRemoteMemory,
    WriteRemoteMemory,
    FlushInstructionCache,
    CreateRemoteThread,
    WaitForRemoteThread,
    ReadRemoteResult,
    ReadAffinity,
    EnumerateWindows,
    ReadWindowMetadata,
    Shutdown,
}

impl OperationStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ValidateWindow => "验证窗口",
            Self::ResolveProcess => "解析所属进程",
            Self::OpenProcess => "打开目标进程",
            Self::DetectArchitecture => "识别目标架构",
            Self::EnumerateModules => "枚举目标模块",
            Self::ReadRemoteImage => "读取目标模块",
            Self::ResolveExport => "解析远程函数",
            Self::AllocateRemoteMemory => "分配远程内存",
            Self::WriteRemoteMemory => "写入远程代码",
            Self::FlushInstructionCache => "刷新指令缓存",
            Self::CreateRemoteThread => "创建远程线程",
            Self::WaitForRemoteThread => "等待远程调用",
            Self::ReadRemoteResult => "读取远程结果",
            Self::ReadAffinity => "读取捕获排除状态",
            Self::EnumerateWindows => "枚举窗口",
            Self::ReadWindowMetadata => "读取窗口信息",
            Self::Shutdown => "关闭后台任务",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationError {
    pub stage: OperationStage,
    pub code: Option<u32>,
    pub context: String,
}

impl OperationError {
    pub fn new(stage: OperationStage, context: impl Into<String>) -> Self {
        Self {
            stage,
            code: None,
            context: context.into(),
        }
    }

    pub fn win32(stage: OperationStage, code: u32, context: impl Into<String>) -> Self {
        Self {
            stage,
            code: Some(code),
            context: context.into(),
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}：{}", self.stage.label(), self.context)?;
        if let Some(code) = self.code {
            write!(formatter, "（Win32 {code} / 0x{code:08X}）")?;
        }
        Ok(())
    }
}

impl std::error::Error for OperationError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(kind: WindowKind) -> WindowMetadata {
        WindowMetadata {
            key: WindowKey::new(WindowHandle::new(0x1234), 42),
            root_handle: WindowHandle::new(0x1234),
            title: "Example".into(),
            class_name: "ExampleClass".into(),
            process_name: "example".into(),
            root_class_name: "ExampleClass".into(),
            root_process_name: "example".into(),
            rule_key: None,
            visible: true,
            cloaked: false,
            top_level: true,
            kind,
            hidden_from_list: false,
            icon: None,
        }
    }

    #[test]
    fn affinity_round_trips_known_and_unknown_values() {
        for raw in [WDA_NONE, WDA_MONITOR, WDA_EXCLUDE_FROM_CAPTURE, 0xCAFE_BABE] {
            assert_eq!(Affinity::from_raw(raw).raw(), raw);
        }
    }

    #[test]
    fn pending_request_controls_displayed_state() {
        let mut window = ManagedWindow {
            row_id: 1,
            metadata: metadata(WindowKind::Normal),
            actual_affinity: Affinity::None,
            policy_enabled: false,
            pending: Some(PendingOperation {
                request_id: 9,
                target: Affinity::ExcludeFromCapture,
                automatic: false,
            }),
            last_failed_target: None,
            was_visible: true,
        };

        assert!(window.displayed_enabled());
        assert_eq!(
            window.protection_state(),
            ProtectionState::ApplyingProtection
        );

        window.pending = Some(PendingOperation {
            request_id: 10,
            target: Affinity::None,
            automatic: false,
        });
        assert!(!window.displayed_enabled());
        assert_eq!(
            window.protection_state(),
            ProtectionState::RemovingProtection
        );
    }

    #[test]
    fn system_policy_and_actual_affinity_remain_distinct() {
        let window = ManagedWindow {
            row_id: 3,
            metadata: metadata(WindowKind::InputMethod),
            actual_affinity: Affinity::None,
            policy_enabled: true,
            pending: None,
            last_failed_target: None,
            was_visible: false,
        };

        assert!(window.displayed_enabled());
        assert_eq!(window.protection_state(), ProtectionState::Unprotected);
    }

    #[test]
    fn failed_target_has_explicit_state() {
        let window = ManagedWindow {
            row_id: 2,
            metadata: metadata(WindowKind::Normal),
            actual_affinity: Affinity::None,
            policy_enabled: false,
            pending: None,
            last_failed_target: Some(Affinity::ExcludeFromCapture),
            was_visible: true,
        };

        assert_eq!(window.protection_state(), ProtectionState::FailedToProtect);
        assert_eq!(window.protection_state().label(), "启用失败");
    }

    #[test]
    fn window_key_distinguishes_reused_handle_between_processes() {
        let first = WindowKey::new(WindowHandle::new(55), 100);
        let reused = WindowKey::new(WindowHandle::new(55), 101);
        assert_ne!(first, reused);
    }

    #[test]
    fn display_title_localizes_special_windows_without_losing_source_title() {
        let mut desktop = metadata(WindowKind::Normal);
        desktop.title = "Program Manager".into();
        assert_eq!(desktop.display_title(), "桌面和图标");
        assert_eq!(desktop.title, "Program Manager");

        let wetype = metadata(WindowKind::WeTypeCandidate);
        assert_eq!(wetype.display_title(), "微信输入法候选框");
    }
    #[test]
    fn window_icon_requires_exact_non_empty_rgba_dimensions() {
        assert!(WindowIcon::from_rgba("valid.exe", 2, 1, vec![0; 8]).is_some());
        assert!(WindowIcon::from_rgba("empty.exe", 0, 1, Vec::new()).is_none());
        assert!(WindowIcon::from_rgba("short.exe", 2, 1, vec![0; 7]).is_none());
    }
}
