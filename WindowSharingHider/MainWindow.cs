using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace WindowSharingHider
{
    public partial class MainWindow : Form
    {
        public class WindowInfo
        {
            public String Title { get; set; }
            public IntPtr Handle { get; set; }
            public IntPtr RootHandle { get; set; }
            public String ClassName { get; set; }
            public String ProcessName { get; set; }
            public String RuleKey { get; set; }
            public Boolean IsSystemCandidate { get; set; }
            public Boolean StillExists { get; set; }
            public Boolean IsVisible { get; set; }
            public Boolean PolicyEnabled { get; set; }
            public Int32 CurrentAffinity { get; set; }
            public Int32? PendingTargetAffinity { get; set; }
            public Int32? LastFailedTargetAffinity { get; set; }
            public Int32 LastApplyRequestId { get; set; }
            public Boolean IsWeTypeCandidate { get; set; }
            public Boolean IsHiddenFromList { get; set; }

            public override string ToString()
            {
                if (IsWeTypeCandidate) return "微信输入法";
                if (!IsSystemCandidate) return Title;

                var title = String.IsNullOrWhiteSpace(Title) ? "(no title)" : Title;
                var processName = String.IsNullOrWhiteSpace(ProcessName) ? "unknown-process" : ProcessName;
                var className = String.IsNullOrWhiteSpace(ClassName) ? "unknown-class" : ClassName;
                return $"[IME] {processName} | {className} | {title}";
            }
        }

        private sealed class WindowSnapshot
        {
            public WindowHandler.WindowEntry Entry { get; set; }
            public Int32 CurrentAffinity { get; set; }
        }

        private sealed class AffinityUpdateResult
        {
            public Int32 RequestId { get; set; }
            public Int32 TargetAffinity { get; set; }
            public Int32 CurrentAffinity { get; set; }
            public Boolean IsAutoRetry { get; set; }
            public WindowHandler.ApplyAffinityResult ApplyResult { get; set; }
        }

        private readonly Timer refreshTimer = new Timer();
        private Boolean isRefreshingUi = false;
        private Boolean refreshInFlight = false;
        private Boolean refreshQueued = false;
        private Int32 nextApplyRequestId = 0;
        private WindowInfo weTypeCandidateWindow;

        public MainWindow()
        {
            InitializeComponent();
            Load += MainWindow_Load;
            windowListCheckBox.ItemCheck += WindowListCheckBox_ItemCheck;
            hideWeTypeCandidateCheckBox.CheckedChanged += HideWeTypeCandidateCheckBox_CheckedChanged;
            showSystemWindowsCheckBox.CheckedChanged += ShowSystemWindowsCheckBox_CheckedChanged;
            refreshTimer.Interval = 1000;
            refreshTimer.Tick += Timer_Tick;
            SetStatus("Ready.");
        }

        private void MainWindow_Load(object sender, EventArgs e)
        {
            MessageBox.Show(
                this,
                "请先勾选一次“隐藏 微信输入法”。\n\n然后打开任意软件，输入任意中文字符并保持候选框显示，再返回本软件重新勾选一次。",
                "提示",
                MessageBoxButtons.OK,
                MessageBoxIcon.Information);
            QueueRefresh();
            refreshTimer.Start();
        }

        private void ShowSystemWindowsCheckBox_CheckedChanged(object sender, EventArgs e)
        {
            SetStatus(showSystemWindowsCheckBox.Checked
                ? "Advanced system / IME window mode enabled."
                : "Advanced system / IME window mode disabled.");
            QueueRefresh();
        }

        private void HideWeTypeCandidateCheckBox_CheckedChanged(object sender, EventArgs e)
        {
            if (weTypeCandidateWindow != null)
            {
                weTypeCandidateWindow.PolicyEnabled = hideWeTypeCandidateCheckBox.Checked;
                var targetAffinity = hideWeTypeCandidateCheckBox.Checked
                    ? WindowHandler.WDA_EXCLUDEFROMCAPTURE
                    : WindowHandler.WDA_NONE;
                BeginAffinityUpdate(weTypeCandidateWindow, targetAffinity, false);
            }
            else
            {
                SetStatus(hideWeTypeCandidateCheckBox.Checked
                    ? "Waiting for WeChat IME (wetype_candidate) to appear."
                    : "WeChat IME capture exclusion disabled.");
            }

            QueueRefresh();
        }

        private void WindowListCheckBox_ItemCheck(object sender, ItemCheckEventArgs e)
        {
            if (isRefreshingUi) return;
            if (e.Index < 0 || e.Index >= windowListCheckBox.Items.Count) return;
            if (!(windowListCheckBox.Items[e.Index] is WindowInfo windowInfo)) return;

            var targetAffinity = e.NewValue == CheckState.Checked
                ? WindowHandler.WDA_EXCLUDEFROMCAPTURE
                : WindowHandler.WDA_NONE;

            BeginAffinityUpdate(windowInfo, targetAffinity, false);
        }

        private void Timer_Tick(object sender, EventArgs e)
        {
            QueueRefresh();
        }

        protected override void OnFormClosing(FormClosingEventArgs e)
        {
            refreshTimer.Stop();
            base.OnFormClosing(e);
        }

        private void QueueRefresh()
        {
            refreshQueued = true;
            if (refreshInFlight) return;

            _ = ProcessRefreshQueueAsync();
        }

        private async Task ProcessRefreshQueueAsync()
        {
            if (refreshInFlight) return;

            refreshInFlight = true;
            try
            {
                try
                {
                    while (refreshQueued && !IsDisposed)
                    {
                        refreshQueued = false;
                        var includeSystemCandidates = showSystemWindowsCheckBox.Checked;
                        var includeWeTypeCandidate = hideWeTypeCandidateCheckBox.Checked;
                        List<WindowSnapshot> snapshot;

                        try
                        {
                            snapshot = await Task.Run(() => CaptureWindowSnapshot(includeSystemCandidates, includeWeTypeCandidate));
                        }
                        catch (Exception ex)
                        {
                            SetStatus("Refresh failed: " + ex.Message);
                            continue;
                        }

                        await InvokeOnUiThreadAsync(() => ApplyWindowSnapshot(snapshot));
                    }
                }
                catch (Exception ex)
                {
                    SetStatus("Refresh failed: " + ex.Message);
                }
            }
            finally
            {
                refreshInFlight = false;
                if (refreshQueued && !IsDisposed) _ = ProcessRefreshQueueAsync();
            }
        }

        private static List<WindowSnapshot> CaptureWindowSnapshot(Boolean includeSystemCandidates, Boolean includeWeTypeCandidate)
        {
            var windows = WindowHandler.GetWindows(includeSystemCandidates, includeWeTypeCandidate);
            var snapshot = new List<WindowSnapshot>(windows.Count);

            foreach (var window in windows)
            {
                var affinityHandle = window.IsSystemWindow
                    ? WindowHandler.GetAffinityProbeHandle(window)
                    : window.Handle;

                snapshot.Add(new WindowSnapshot
                {
                    Entry = window,
                    CurrentAffinity = WindowHandler.GetWindowDisplayAffinityValue(affinityHandle)
                });
            }

            return snapshot;
        }

        private void ApplyWindowSnapshot(List<WindowSnapshot> snapshot)
        {
            var visibleSnapshot = snapshot.Where(window => !window.Entry.IsHiddenFromList).ToList();
            var weTypeSnapshot = snapshot.LastOrDefault(window => window.Entry.IsWeTypeCandidate);
            var existingWindows = windowListCheckBox.Items.Cast<WindowInfo>().ToList();
            var existingByHandle = existingWindows.ToDictionary(window => window.Handle);
            var existingSystemByRuleKey = existingWindows
                .Where(window => window.IsSystemCandidate && !String.IsNullOrWhiteSpace(window.RuleKey))
                .GroupBy(window => window.RuleKey, StringComparer.OrdinalIgnoreCase)
                .ToDictionary(group => group.Key, group => group.First(), StringComparer.OrdinalIgnoreCase);

            foreach (var window in existingWindows) window.StillExists = false;

            windowListCheckBox.BeginUpdate();
            try
            {
                foreach (var windowSnapshot in visibleSnapshot)
                {
                    if (existingByHandle.TryGetValue(windowSnapshot.Entry.Handle, out WindowInfo existingWindow))
                    {
                        var wasVisible = existingWindow.IsVisible;
                        UpdateWindowInfo(existingWindow, windowSnapshot.Entry, windowSnapshot.CurrentAffinity);
                        QueueAutoRetryIfNeeded(existingWindow, wasVisible);
                    }
                    else
                    {
                        var windowInfo = CreateWindowInfo(windowSnapshot.Entry, windowSnapshot.CurrentAffinity);
                        if (windowInfo.IsSystemCandidate
                            && !String.IsNullOrWhiteSpace(windowInfo.RuleKey)
                            && existingSystemByRuleKey.TryGetValue(windowInfo.RuleKey, out WindowInfo previousWindow))
                        {
                            CopySystemWindowState(previousWindow, windowInfo);
                        }

                        windowListCheckBox.Items.Add(windowInfo);
                        QueueAutoRetryIfNeeded(windowInfo, false);
                    }
                }

                foreach (var window in windowListCheckBox.Items.Cast<WindowInfo>().ToArray())
                {
                    if (!window.StillExists) windowListCheckBox.Items.Remove(window);
                }

                foreach (var window in windowListCheckBox.Items.Cast<WindowInfo>().ToArray())
                {
                    SetCheckedState(window, GetDisplayedEnabledState(window));
                }
            }
            finally
            {
                windowListCheckBox.EndUpdate();
            }

            ApplyWeTypeCandidateSnapshot(weTypeSnapshot);
        }

        private void BeginAffinityUpdate(WindowInfo windowInfo, Int32 targetAffinity, Boolean isAutoRetry)
        {
            if (windowInfo.IsSystemCandidate)
            {
                windowInfo.PolicyEnabled = targetAffinity > 0;
            }

            windowInfo.LastFailedTargetAffinity = null;
            windowInfo.PendingTargetAffinity = targetAffinity;
            var requestId = ++nextApplyRequestId;
            windowInfo.LastApplyRequestId = requestId;

            if (!isAutoRetry)
            {
                SetStatus(targetAffinity == WindowHandler.WDA_EXCLUDEFROMCAPTURE
                    ? $"Applying capture exclusion for {windowInfo}..."
                    : $"Removing capture exclusion for {windowInfo}...");
            }

            _ = ApplyAffinityAsync(windowInfo, targetAffinity, requestId, isAutoRetry);
        }

        private async Task ApplyAffinityAsync(WindowInfo windowInfo, Int32 targetAffinity, Int32 requestId, Boolean isAutoRetry)
        {
            try
            {
                var result = await Task.Run(() =>
                {
                    var applyResult = WindowHandler.ApplyWindowProtection(CreateWindowEntry(windowInfo), targetAffinity);
                    return new AffinityUpdateResult
                    {
                        RequestId = requestId,
                        TargetAffinity = targetAffinity,
                        CurrentAffinity = applyResult.CurrentAffinity,
                        IsAutoRetry = isAutoRetry,
                        ApplyResult = applyResult
                    };
                });

                await InvokeOnUiThreadAsync(() => HandleAffinityUpdateResult(windowInfo, result));
            }
            catch (Exception ex)
            {
                await InvokeOnUiThreadAsync(() =>
                {
                    windowInfo.PendingTargetAffinity = null;
                    windowInfo.LastFailedTargetAffinity = targetAffinity;
                    if (windowInfo.IsSystemCandidate)
                    {
                        windowInfo.PolicyEnabled = false;
                    }

                    SetStatus($"Failed to update {windowInfo}: {ex.Message}");
                    SetCheckedState(windowInfo, GetDisplayedEnabledState(windowInfo));
                    QueueRefresh();
                });
            }
        }

        private void HandleAffinityUpdateResult(WindowInfo windowInfo, AffinityUpdateResult result)
        {
            if (windowInfo.LastApplyRequestId != result.RequestId) return;

            windowInfo.PendingTargetAffinity = null;
            windowInfo.CurrentAffinity = result.CurrentAffinity;

            if (result.ApplyResult.Success)
            {
                windowInfo.LastFailedTargetAffinity = null;
                if (windowInfo.IsSystemCandidate)
                {
                    windowInfo.PolicyEnabled = result.TargetAffinity > 0;
                }

                if (!result.IsAutoRetry)
                {
                    SetStatus(result.TargetAffinity == WindowHandler.WDA_EXCLUDEFROMCAPTURE
                        ? $"Applied capture exclusion for {windowInfo}."
                        : $"Removed capture exclusion for {windowInfo}.");
                }
            }
            else
            {
                windowInfo.LastFailedTargetAffinity = result.TargetAffinity;
                if (windowInfo.IsSystemCandidate)
                {
                    windowInfo.PolicyEnabled = windowInfo.CurrentAffinity > 0;
                }

                SetStatus($"Failed to update {windowInfo}: {result.ApplyResult.ErrorMessage}");
            }

            SetCheckedState(windowInfo, GetDisplayedEnabledState(windowInfo));
            QueueRefresh();
        }

        private WindowInfo CreateWindowInfo(WindowHandler.WindowEntry window, Int32 currentAffinity)
        {
            var windowInfo = new WindowInfo();
            UpdateWindowInfo(windowInfo, window, currentAffinity);
            windowInfo.StillExists = true;
            return windowInfo;
        }

        private void UpdateWindowInfo(WindowInfo windowInfo, WindowHandler.WindowEntry window, Int32 currentAffinity)
        {
            windowInfo.Title = window.Title;
            windowInfo.Handle = window.Handle;
            windowInfo.RootHandle = window.RootHandle;
            windowInfo.ClassName = window.ClassName;
            windowInfo.ProcessName = window.ProcessName;
            windowInfo.RuleKey = window.RuleKey;
            windowInfo.IsSystemCandidate = window.IsSystemWindow;
            windowInfo.IsWeTypeCandidate = window.IsWeTypeCandidate;
            windowInfo.IsHiddenFromList = window.IsHiddenFromList;
            windowInfo.StillExists = true;
            windowInfo.IsVisible = window.IsVisible;
            windowInfo.CurrentAffinity = currentAffinity;

            if (!windowInfo.IsSystemCandidate)
            {
                windowInfo.PolicyEnabled = currentAffinity > 0;
            }
        }

        private void SetCheckedState(WindowInfo window, Boolean isChecked)
        {
            var index = windowListCheckBox.Items.IndexOf(window);
            if (index < 0) return;

            isRefreshingUi = true;
            try
            {
                windowListCheckBox.SetItemChecked(index, isChecked);
            }
            finally
            {
                isRefreshingUi = false;
            }
        }

        private void SetStatus(String message)
        {
            statusLabel.Text = message;
        }

        private Boolean GetDisplayedEnabledState(WindowInfo windowInfo)
        {
            if (windowInfo.PendingTargetAffinity.HasValue)
            {
                return windowInfo.PendingTargetAffinity.Value > 0;
            }

            if (windowInfo.IsSystemCandidate)
            {
                return windowInfo.PolicyEnabled;
            }

            return windowInfo.CurrentAffinity > 0;
        }

        private void CopySystemWindowState(WindowInfo source, WindowInfo target)
        {
            target.PolicyEnabled = source.PolicyEnabled;
            target.LastFailedTargetAffinity = source.LastFailedTargetAffinity;
        }

        private void ApplyWeTypeCandidateSnapshot(WindowSnapshot windowSnapshot)
        {
            if (windowSnapshot == null)
            {
                weTypeCandidateWindow = null;
                return;
            }

            var wasVisible = weTypeCandidateWindow?.IsVisible ?? false;
            if (weTypeCandidateWindow == null)
            {
                weTypeCandidateWindow = CreateWindowInfo(windowSnapshot.Entry, windowSnapshot.CurrentAffinity);
            }
            else
            {
                UpdateWindowInfo(weTypeCandidateWindow, windowSnapshot.Entry, windowSnapshot.CurrentAffinity);
            }

            weTypeCandidateWindow.PolicyEnabled = hideWeTypeCandidateCheckBox.Checked;
            QueueDedicatedPolicyUpdateIfNeeded(weTypeCandidateWindow, wasVisible);
        }

        private void QueueAutoRetryIfNeeded(WindowInfo windowInfo, Boolean wasVisible)
        {
            if (!windowInfo.IsSystemCandidate) return;
            if (!windowInfo.PolicyEnabled) return;
            if (windowInfo.PendingTargetAffinity.HasValue) return;
            if (windowInfo.LastFailedTargetAffinity == WindowHandler.WDA_EXCLUDEFROMCAPTURE) return;
            if (!windowInfo.IsVisible) return;
            if (wasVisible) return;

            BeginAffinityUpdate(windowInfo, WindowHandler.WDA_EXCLUDEFROMCAPTURE, true);
        }

        private void QueueDedicatedPolicyUpdateIfNeeded(WindowInfo windowInfo, Boolean wasVisible)
        {
            if (windowInfo == null) return;
            if (windowInfo.PendingTargetAffinity.HasValue) return;

            var targetAffinity = hideWeTypeCandidateCheckBox.Checked
                ? WindowHandler.WDA_EXCLUDEFROMCAPTURE
                : WindowHandler.WDA_NONE;
            if (windowInfo.CurrentAffinity == targetAffinity)
            {
                return;
            }

            if (windowInfo.LastFailedTargetAffinity == targetAffinity) return;
            if (targetAffinity == WindowHandler.WDA_EXCLUDEFROMCAPTURE && !windowInfo.IsVisible && !wasVisible) return;

            BeginAffinityUpdate(windowInfo, targetAffinity, true);
        }

        private WindowHandler.WindowEntry CreateWindowEntry(WindowInfo windowInfo)
        {
            return new WindowHandler.WindowEntry
            {
                Handle = windowInfo.Handle,
                RootHandle = windowInfo.RootHandle,
                Title = windowInfo.Title,
                ClassName = windowInfo.ClassName,
                ProcessName = windowInfo.ProcessName,
                RuleKey = windowInfo.RuleKey,
                IsVisible = windowInfo.IsVisible,
                IsSystemWindow = windowInfo.IsSystemCandidate,
                IsImeCandidate = windowInfo.IsSystemCandidate,
                IsTopLevel = windowInfo.Handle == windowInfo.RootHandle,
                IsWeTypeCandidate = windowInfo.IsWeTypeCandidate,
                IsHiddenFromList = windowInfo.IsHiddenFromList
            };
        }

        private Task InvokeOnUiThreadAsync(Action action)
        {
            if (IsDisposed || Disposing) return Task.CompletedTask;
            if (InvokeRequired)
            {
                var completion = new TaskCompletionSource<Boolean>();
                BeginInvoke((Action)(() =>
                {
                    if (IsDisposed || Disposing)
                    {
                        completion.TrySetResult(true);
                        return;
                    }

                    try
                    {
                        action();
                        completion.TrySetResult(true);
                    }
                    catch (Exception ex)
                    {
                        completion.TrySetException(ex);
                    }
                }));
                return completion.Task;
            }

            action();
            return Task.CompletedTask;
        }
    }
}
