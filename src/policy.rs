use crate::model::{Affinity, ManagedWindow, WindowHandle, WindowKind, WindowMetadata};

pub const WETYPE_CANDIDATE_NAME: &str = "wetype_candidate";

const SYSTEM_PROCESS_NAMES: &[&str] = &[
    "TextInputHost",
    "ApplicationFrameHost",
    "explorer",
    "ctfmon",
    "TabTip",
];
const HOST_CLASS_NAMES: &[&str] = &["Windows.UI.Core.CoreWindow", "EdgeUiInputTopWndClass"];
const IME_CLASS_NAMES: &[&str] = &["IME", "MSCTFIME UI", "CiceroUIWndFrame"];
const EMBEDDED_IME_CLASS_NAMES: &[&str] = &[
    "ApplicationFrameInputSinkWindow",
    "Windows.UI.Input.InputSite.WindowClass",
    "InputSiteWindowClass",
    "InputNonClientPointerSource",
];
const GENERIC_CONTAINER_ROOT_CLASS_NAMES: &[&str] = &["Shell_TrayWnd", "ApplicationFrameWindow"];

#[derive(Clone, Copy, Debug)]
pub struct WindowDescriptor<'a> {
    pub process_name: &'a str,
    pub class_name: &'a str,
    pub title: &'a str,
    pub root_process_name: &'a str,
    pub root_class_name: &'a str,
    pub top_level: bool,
}

fn contains_ignore_ascii_case(values: &[&str], candidate: &str) -> bool {
    values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(candidate))
}

pub fn matches_wetype_candidate_name(value: &str) -> bool {
    value.eq_ignore_ascii_case(WETYPE_CANDIDATE_NAME)
}

pub fn is_system_process(process_name: &str) -> bool {
    contains_ignore_ascii_case(SYSTEM_PROCESS_NAMES, process_name)
}

pub fn is_system_candidate_window(process_name: &str, class_name: &str, top_level: bool) -> bool {
    if matches_wetype_candidate_name(process_name) || matches_wetype_candidate_name(class_name) {
        return true;
    }
    if process_name.eq_ignore_ascii_case("TextInputHost") && top_level {
        return true;
    }
    if process_name.eq_ignore_ascii_case("TabTip") && top_level {
        return true;
    }
    if process_name.eq_ignore_ascii_case("ctfmon")
        && contains_ignore_ascii_case(IME_CLASS_NAMES, class_name)
    {
        return true;
    }
    if process_name.eq_ignore_ascii_case("ApplicationFrameHost")
        && (contains_ignore_ascii_case(HOST_CLASS_NAMES, class_name)
            || contains_ignore_ascii_case(EMBEDDED_IME_CLASS_NAMES, class_name))
    {
        return true;
    }
    if process_name.eq_ignore_ascii_case("explorer")
        && (class_name.eq_ignore_ascii_case("EdgeUiInputTopWndClass")
            || contains_ignore_ascii_case(EMBEDDED_IME_CLASS_NAMES, class_name))
    {
        return true;
    }

    is_system_process(process_name)
        && (contains_ignore_ascii_case(HOST_CLASS_NAMES, class_name)
            || contains_ignore_ascii_case(IME_CLASS_NAMES, class_name)
            || contains_ignore_ascii_case(EMBEDDED_IME_CLASS_NAMES, class_name))
}

pub fn is_system_candidate_child(process_name: &str, class_name: &str) -> bool {
    !class_name.trim().is_empty()
        && (matches_wetype_candidate_name(process_name)
            || matches_wetype_candidate_name(class_name)
            || contains_ignore_ascii_case(IME_CLASS_NAMES, class_name)
            || contains_ignore_ascii_case(HOST_CLASS_NAMES, class_name)
            || contains_ignore_ascii_case(EMBEDDED_IME_CLASS_NAMES, class_name)
            || (process_name.eq_ignore_ascii_case("explorer")
                && class_name.eq_ignore_ascii_case("EdgeUiInputTopWndClass")))
}

pub fn classify_window(descriptor: WindowDescriptor<'_>) -> WindowKind {
    let wetype = matches_wetype_candidate_name(descriptor.process_name)
        || matches_wetype_candidate_name(descriptor.class_name)
        || matches_wetype_candidate_name(descriptor.title)
        || (!descriptor.top_level
            && (matches_wetype_candidate_name(descriptor.root_process_name)
                || matches_wetype_candidate_name(descriptor.root_class_name)));
    if wetype {
        return WindowKind::WeTypeCandidate;
    }

    let direct_system = is_system_candidate_window(
        descriptor.process_name,
        descriptor.class_name,
        descriptor.top_level,
    );
    let hosted_child = !descriptor.top_level
        && is_system_candidate_window(
            descriptor.root_process_name,
            descriptor.root_class_name,
            true,
        )
        && is_system_candidate_child(descriptor.process_name, descriptor.class_name);

    if direct_system || hosted_child {
        if contains_ignore_ascii_case(IME_CLASS_NAMES, descriptor.class_name)
            || contains_ignore_ascii_case(EMBEDDED_IME_CLASS_NAMES, descriptor.class_name)
            || descriptor.process_name.eq_ignore_ascii_case("ctfmon")
        {
            WindowKind::InputMethod
        } else {
            WindowKind::System
        }
    } else {
        WindowKind::Normal
    }
}

pub fn create_rule_key(metadata: &WindowMetadata) -> Option<String> {
    match metadata.kind {
        WindowKind::Normal => None,
        WindowKind::WeTypeCandidate => Some(WETYPE_CANDIDATE_NAME.into()),
        WindowKind::System | WindowKind::InputMethod => Some(format!(
            "{}|{}|{}|{}",
            metadata.root_process_name,
            metadata.root_class_name,
            metadata.process_name,
            metadata.class_name
        )),
    }
}

pub fn should_include(
    metadata: &WindowMetadata,
    include_system_candidates: bool,
    include_wetype_candidate: bool,
) -> bool {
    match metadata.kind {
        WindowKind::WeTypeCandidate => include_wetype_candidate,
        WindowKind::System | WindowKind::InputMethod => include_system_candidates,
        WindowKind::Normal => {
            metadata.visible && !metadata.cloaked && !metadata.title.trim().is_empty()
        }
    }
}

pub fn should_scan_children(
    metadata: &WindowMetadata,
    include_system_candidates: bool,
    include_wetype_candidate: bool,
) -> bool {
    metadata.top_level
        && (metadata.kind.is_system_candidate()
            || (include_system_candidates && is_system_process(&metadata.process_name))
            || (include_wetype_candidate && (metadata.visible || !metadata.cloaked)))
}

pub fn affinity_targets(metadata: &WindowMetadata) -> Vec<WindowHandle> {
    let mut targets = Vec::with_capacity(2);
    if !metadata.key.handle.is_null() {
        targets.push(metadata.key.handle);
    }

    let can_use_root = !metadata.top_level
        && !metadata.root_handle.is_null()
        && metadata.root_handle != metadata.key.handle
        && !contains_ignore_ascii_case(
            GENERIC_CONTAINER_ROOT_CLASS_NAMES,
            &metadata.root_class_name,
        )
        && is_system_candidate_window(&metadata.root_process_name, &metadata.root_class_name, true);
    if can_use_root {
        targets.push(metadata.root_handle);
    }
    targets
}

pub fn should_auto_retry(window: &ManagedWindow, was_visible: bool) -> bool {
    window.metadata.kind.is_system_candidate()
        && window.policy_enabled
        && window.pending.is_none()
        && window.last_failed_target != Some(Affinity::ExcludeFromCapture)
        && window.metadata.visible
        && !was_visible
}

pub fn should_apply_dedicated_policy(
    window: &ManagedWindow,
    was_visible: bool,
    enabled: bool,
) -> bool {
    if window.metadata.kind != WindowKind::WeTypeCandidate || window.pending.is_some() {
        return false;
    }

    let target = Affinity::requested(enabled);
    window.actual_affinity != target
        && window.last_failed_target != Some(target)
        && (!enabled || window.metadata.visible || was_visible)
}

pub fn migrate_system_policy(source: &ManagedWindow, target: &mut ManagedWindow) {
    if !source.metadata.kind.is_system_candidate()
        || !target.metadata.kind.is_system_candidate()
        || source.metadata.rule_key.is_none()
        || source.metadata.rule_key != target.metadata.rule_key
    {
        return;
    }

    target.policy_enabled = source.policy_enabled;
    target.last_failed_target = source.last_failed_target;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PendingOperation, WindowKey};

    fn metadata(
        process_name: &str,
        class_name: &str,
        title: &str,
        top_level: bool,
    ) -> WindowMetadata {
        let root_handle = WindowHandle::new(10);
        let mut value = WindowMetadata {
            key: WindowKey::new(WindowHandle::new(if top_level { 10 } else { 11 }), 5),
            root_handle,
            title: title.into(),
            class_name: class_name.into(),
            process_name: process_name.into(),
            root_class_name: if top_level {
                class_name.into()
            } else {
                "Windows.UI.Core.CoreWindow".into()
            },
            root_process_name: if top_level {
                process_name.into()
            } else {
                "ApplicationFrameHost".into()
            },
            rule_key: None,
            visible: true,
            cloaked: false,
            top_level,
            kind: WindowKind::Normal,
            hidden_from_list: false,
            icon: None,
        };
        value.kind = classify_window(WindowDescriptor {
            process_name: &value.process_name,
            class_name: &value.class_name,
            title: &value.title,
            root_process_name: &value.root_process_name,
            root_class_name: &value.root_class_name,
            top_level: value.top_level,
        });
        value.rule_key = create_rule_key(&value);
        value.hidden_from_list = value.kind == WindowKind::WeTypeCandidate;
        value
    }

    fn managed(metadata: WindowMetadata) -> ManagedWindow {
        ManagedWindow {
            row_id: 1,
            metadata,
            actual_affinity: Affinity::None,
            policy_enabled: false,
            pending: None,
            last_failed_target: None,
            was_visible: false,
        }
    }

    #[test]
    fn classifies_supported_system_and_ime_windows() {
        let cases = [
            ("TextInputHost", "Anything", true, WindowKind::System),
            ("TabTip", "IPTip_Main_Window", true, WindowKind::System),
            ("ctfmon", "IME", false, WindowKind::InputMethod),
            (
                "ApplicationFrameHost",
                "Windows.UI.Core.CoreWindow",
                true,
                WindowKind::System,
            ),
            (
                "explorer",
                "Windows.UI.Input.InputSite.WindowClass",
                false,
                WindowKind::InputMethod,
            ),
        ];

        for (process, class_name, top_level, expected) in cases {
            assert_eq!(
                metadata(process, class_name, "title", top_level).kind,
                expected
            );
        }
    }

    #[test]
    fn wetype_match_is_exact_and_case_insensitive() {
        assert_eq!(
            metadata("WeType_Candidate", "Other", "", true).kind,
            WindowKind::WeTypeCandidate
        );
        assert_eq!(
            metadata("prefix_wetype_candidate", "Other", "", true).kind,
            WindowKind::Normal
        );
    }

    #[test]
    fn normal_window_requires_visibility_title_and_no_cloak() {
        let mut window = metadata("notepad", "Notepad", "Document", true);
        assert!(should_include(&window, false, false));
        window.visible = false;
        assert!(!should_include(&window, false, false));
        window.visible = true;
        window.cloaked = true;
        assert!(!should_include(&window, false, false));
        window.cloaked = false;
        window.title.clear();
        assert!(!should_include(&window, false, false));
    }

    #[test]
    fn candidate_modes_are_independent() {
        let system = metadata("TextInputHost", "Host", "", true);
        let wetype = metadata("wetype_candidate", "Other", "", true);
        assert!(should_include(&system, true, false));
        assert!(!should_include(&system, false, true));
        assert!(should_include(&wetype, false, true));
        assert!(!should_include(&wetype, true, false));
    }

    #[test]
    fn generic_container_root_is_never_an_affinity_fallback() {
        let mut child = metadata("ctfmon", "IME", "candidate", false);
        child.root_class_name = "ApplicationFrameWindow".into();
        child.root_process_name = "ApplicationFrameHost".into();
        assert_eq!(affinity_targets(&child), vec![child.key.handle]);
    }

    #[test]
    fn valid_system_host_is_second_affinity_target() {
        let child = metadata("ctfmon", "IME", "candidate", false);
        assert_eq!(
            affinity_targets(&child),
            vec![child.key.handle, child.root_handle]
        );
    }

    #[test]
    fn auto_retry_requires_newly_visible_enabled_candidate() {
        let mut window = managed(metadata("ctfmon", "IME", "candidate", false));
        window.policy_enabled = true;
        assert!(should_auto_retry(&window, false));
        assert!(!should_auto_retry(&window, true));
        window.last_failed_target = Some(Affinity::ExcludeFromCapture);
        assert!(!should_auto_retry(&window, false));
        window.last_failed_target = None;
        window.pending = Some(PendingOperation {
            request_id: 1,
            target: Affinity::ExcludeFromCapture,
            automatic: true,
        });
        assert!(!should_auto_retry(&window, false));
    }

    #[test]
    fn dedicated_policy_waits_until_candidate_is_visible() {
        let mut window = managed(metadata("wetype_candidate", "Other", "", true));
        window.metadata.visible = false;
        assert!(!should_apply_dedicated_policy(&window, false, true));
        window.metadata.visible = true;
        assert!(should_apply_dedicated_policy(&window, false, true));
        window.last_failed_target = Some(Affinity::ExcludeFromCapture);
        assert!(!should_apply_dedicated_policy(&window, false, true));
    }

    #[test]
    fn policy_migrates_only_between_equal_system_rule_keys() {
        let mut source = managed(metadata("ctfmon", "IME", "one", false));
        source.policy_enabled = true;
        source.last_failed_target = Some(Affinity::ExcludeFromCapture);

        let mut replacement = managed(metadata("ctfmon", "IME", "two", false));
        replacement.metadata.key = WindowKey::new(WindowHandle::new(99), 5);
        migrate_system_policy(&source, &mut replacement);
        assert!(replacement.policy_enabled);
        assert_eq!(replacement.last_failed_target, source.last_failed_target);

        replacement.metadata.rule_key = Some("different".into());
        replacement.policy_enabled = false;
        migrate_system_policy(&source, &mut replacement);
        assert!(!replacement.policy_enabled);
    }
}
