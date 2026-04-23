using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

namespace WindowSharingHider
{
    public static class WindowHandler
    {
        public const Int32 WDA_NONE = 0x0;
        public const Int32 WDA_EXCLUDEFROMCAPTURE = 0x11;
        public const String WETYPE_CANDIDATE_NAME = "wetype_candidate";
        private const Int32 DWMWA_CLOAKED = 14;
        private const Int32 GA_ROOT = 2;
        private const Int32 MAX_CLASS_NAME = 256;
        private const Int32 PROCESS_CREATE_THREAD = 0x0002;
        private const Int32 PROCESS_VM_OPERATION = 0x0008;
        private const Int32 PROCESS_VM_READ = 0x0010;
        private const Int32 PROCESS_VM_WRITE = 0x0020;
        private const Int32 PROCESS_QUERY_INFORMATION = 0x0400;
        private const Int32 REMOTE_THREAD_TIMEOUT_MS = 10000;

        private static readonly HashSet<String> SystemProcessNames = new HashSet<String>(StringComparer.OrdinalIgnoreCase)
        {
            "TextInputHost",
            "ApplicationFrameHost",
            "explorer",
            "ctfmon",
            "TabTip"
        };

        private static readonly HashSet<String> HostClassNames = new HashSet<String>(StringComparer.OrdinalIgnoreCase)
        {
            "Windows.UI.Core.CoreWindow",
            "EdgeUiInputTopWndClass"
        };

        private static readonly HashSet<String> ImeClassNames = new HashSet<String>(StringComparer.OrdinalIgnoreCase)
        {
            "IME",
            "MSCTFIME UI",
            "CiceroUIWndFrame"
        };

        private static readonly HashSet<String> InputSiteClassNames = new HashSet<String>(StringComparer.OrdinalIgnoreCase)
        {
            "Windows.UI.Input.InputSite.WindowClass",
            "InputSiteWindowClass",
            "InputNonClientPointerSource"
        };

        private static readonly HashSet<String> EmbeddedImeClassNames = new HashSet<String>(StringComparer.OrdinalIgnoreCase)
        {
            "ApplicationFrameInputSinkWindow",
            "Windows.UI.Input.InputSite.WindowClass",
            "InputSiteWindowClass",
            "InputNonClientPointerSource"
        };

        private static readonly HashSet<String> GenericContainerRootClassNames = new HashSet<String>(StringComparer.OrdinalIgnoreCase)
        {
            "Shell_TrayWnd",
            "ApplicationFrameWindow"
        };

        [DllImport("user32")] private static extern Boolean EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
        [DllImport("user32")] private static extern Boolean EnumChildWindows(IntPtr hWndParent, EnumWindowsProc lpEnumFunc, IntPtr lParam);
        [DllImport("user32")] private static extern Boolean IsWindowVisible(IntPtr hWnd);
        [DllImport("user32")] private static extern Boolean IsWindow(IntPtr hWnd);
        [DllImport("user32")] private static extern IntPtr GetAncestor(IntPtr hWnd, UInt32 gaFlags);
        [DllImport("dwmapi.dll")] private static extern Int32 DwmGetWindowAttribute(IntPtr hwnd, Int32 dwAttribute, out Int32 pvAttribute, Int32 cbAttribute);
        [DllImport("user32")] private static extern IntPtr GetWindowText(IntPtr hWnd, StringBuilder lpString, Int32 nMaxCount);
        [DllImport("user32")] private static extern Int32 GetWindowTextLength(IntPtr hWnd);
        [DllImport("user32", SetLastError = true)] private static extern Int32 GetClassName(IntPtr hWnd, StringBuilder lpClassName, Int32 nMaxCount);
        [DllImport("user32", SetLastError = true)] private static extern Boolean GetWindowDisplayAffinity(IntPtr hWnd, out Int32 dwAffinity);
        [DllImport("user32", SetLastError = true)] private static extern UInt32 GetWindowThreadProcessId(IntPtr hWnd, out Int32 processId);

        [DllImport("kernel32", SetLastError = true)] private static extern IntPtr OpenProcess(Int32 dwDesiredAccess, Boolean bInheritHandle, Int32 dwProcessId);
        [DllImport("kernel32", SetLastError = true)] private static extern IntPtr VirtualAllocEx(IntPtr hProcess, IntPtr lpAddress, Int32 dwSize, UInt32 flAllocationType, UInt32 flProtect);
        [DllImport("kernel32", SetLastError = true)] private static extern Boolean ReadProcessMemory(IntPtr hProcess, UInt64 lpBaseAddress, [In, Out] Byte[] buffer, Int32 size, out Int32 lpNumberOfBytesRead);
        [DllImport("kernel32", SetLastError = true)] private static extern Boolean WriteProcessMemory(IntPtr hProcess, IntPtr lpBaseAddress, Byte[] lpBuffer, Int32 nSize, out Int32 lpNumberOfBytesWritten);
        [DllImport("kernel32", SetLastError = true)] private static extern IntPtr CreateRemoteThread(IntPtr hProcess, IntPtr lpThreadAttributes, UInt32 dwStackSize, IntPtr lpStartAddress, IntPtr lpParameter, UInt32 dwCreationFlags, IntPtr lpThreadId);
        [DllImport("kernel32", SetLastError = true)] private static extern UInt32 WaitForSingleObject(IntPtr hHandle, UInt32 dwMilliseconds);
        [DllImport("kernel32", SetLastError = true)] private static extern Int32 CloseHandle(IntPtr hObject);
        [DllImport("kernel32", SetLastError = true)] private static extern Boolean VirtualFreeEx(IntPtr hProcess, IntPtr lpAddress, Int32 dwSize, Int32 dwFreeType);
        [DllImport("kernel32", SetLastError = true)] private static extern Boolean IsWow64Process(IntPtr processHandle, out Boolean wow64Process);
        [DllImport("kernel32", SetLastError = true)] private static extern Boolean GetExitCodeThread(IntPtr hThread, out UInt32 lpExitCode);

        [DllImport("psapi", SetLastError = true)] private static extern bool GetModuleInformation(IntPtr hProcess, IntPtr hModule, out MODULEINFO lpmodinfo, UInt32 cb);
        [DllImport("psapi", SetLastError = true)] private static extern bool EnumProcessModulesEx(IntPtr hProcess, [MarshalAs(UnmanagedType.LPArray, ArraySubType = UnmanagedType.U4)][In][Out] IntPtr[] lphModule, UInt32 cb, out UInt32 lpcbNeeded, UInt32 dwFilterFlag);
        [DllImport("psapi", SetLastError = true)] private static extern uint GetModuleFileNameEx(IntPtr hProcess, IntPtr hModule, [Out] StringBuilder lpBaseName, UInt32 nSize);

        private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

        [StructLayout(LayoutKind.Sequential)]
        public struct MODULEINFO
        {
            public IntPtr lpBaseOfDll;
            public UInt32 SizeOfImage;
            public IntPtr EntryPoint;
        }

        public sealed class WindowEntry
        {
            public IntPtr Handle { get; set; }
            public IntPtr RootHandle { get; set; }
            public String Title { get; set; }
            public String ClassName { get; set; }
            public String ProcessName { get; set; }
            public String RootClassName { get; set; }
            public String RootProcessName { get; set; }
            public String RuleKey { get; set; }
            public Boolean IsVisible { get; set; }
            public Boolean IsCloaked { get; set; }
            public Boolean IsTopLevel { get; set; }
            public Boolean IsImeCandidate { get; set; }
            public Boolean IsSystemWindow { get; set; }
            public Boolean IsWeTypeCandidate { get; set; }
            public Boolean IsHiddenFromList { get; set; }
        }

        public sealed class ApplyAffinityResult
        {
            public Boolean Success { get; set; }
            public String ErrorMessage { get; set; }
            public Int32 CurrentAffinity { get; set; }
            public Int32 AffectedWindowCount { get; set; }

            public static ApplyAffinityResult Ok(Int32 currentAffinity = WDA_NONE, Int32 affectedWindowCount = 0)
            {
                return new ApplyAffinityResult
                {
                    Success = true,
                    ErrorMessage = String.Empty,
                    CurrentAffinity = currentAffinity,
                    AffectedWindowCount = affectedWindowCount
                };
            }

            public static ApplyAffinityResult Fail(String errorMessage, Int32 currentAffinity = WDA_NONE)
            {
                return new ApplyAffinityResult
                {
                    Success = false,
                    ErrorMessage = errorMessage,
                    CurrentAffinity = currentAffinity,
                    AffectedWindowCount = 0
                };
            }
        }

        public static List<WindowEntry> GetWindows(Boolean includeSystemCandidates, Boolean includeWeTypeCandidate)
        {
            var windows = new Dictionary<IntPtr, WindowEntry>();
            var childScanRoots = new HashSet<IntPtr>();

            EnumWindows(delegate (IntPtr hWnd, IntPtr lParam)
            {
                var entry = BuildWindowEntry(hWnd);
                if (entry == null) return true;

                if (ShouldIncludeWindow(entry, includeSystemCandidates, includeWeTypeCandidate))
                {
                    AddWindowEntry(windows, entry);
                }

                if (ShouldScanChildWindows(entry, includeSystemCandidates, includeWeTypeCandidate))
                {
                    childScanRoots.Add(entry.RootHandle);
                }

                return true;
            }, IntPtr.Zero);

            if (includeSystemCandidates || includeWeTypeCandidate)
            {
                foreach (var rootHandle in childScanRoots)
                {
                    if (rootHandle == IntPtr.Zero || !IsWindow(rootHandle)) continue;

                    EnumChildWindows(rootHandle, delegate (IntPtr childHandle, IntPtr lParam)
                    {
                        var childEntry = BuildWindowEntry(childHandle, rootHandle);
                        if (childEntry == null) return true;

                        if (ShouldIncludeWindow(childEntry, includeSystemCandidates, includeWeTypeCandidate))
                        {
                            AddWindowEntry(windows, childEntry);
                        }

                        return true;
                    }, IntPtr.Zero);
                }
            }

            return new List<WindowEntry>(windows.Values);
        }

        public static Int32 GetWindowDisplayAffinityValue(IntPtr hWnd)
        {
            return GetWindowDisplayAffinity(hWnd, out Int32 dwAffinity) ? dwAffinity : WDA_NONE;
        }

        public static IntPtr GetAffinityProbeHandle(WindowEntry entry)
        {
            if (entry == null) return IntPtr.Zero;

            foreach (var handle in EnumerateAffinityTargets(entry))
            {
                return handle;
            }

            return IntPtr.Zero;
        }

        public static ApplyAffinityResult ApplyWindowProtection(WindowEntry entry, Int32 dwAffinity)
        {
            if (entry == null)
            {
                return ApplyAffinityResult.Fail("No window metadata is available for the selected entry.");
            }

            if (!entry.IsSystemWindow)
            {
                var directResult = SetWindowDisplayAffinity(entry.Handle, dwAffinity);
                directResult.CurrentAffinity = GetWindowDisplayAffinityValue(entry.Handle);
                directResult.AffectedWindowCount = directResult.Success ? 1 : 0;
                return directResult;
            }

            return ApplySystemWindowProtection(entry, dwAffinity);
        }

        public static ApplyAffinityResult SetWindowDisplayAffinity(IntPtr hWnd, Int32 dwAffinity)
        {
            if (GetWindowThreadProcessId(hWnd, out Int32 procId) == 0 || procId == 0)
            {
                return ApplyAffinityResult.Fail("Unable to resolve the owning process for the selected window.");
            }

            var desiredAccess = PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_WRITE | PROCESS_VM_READ;
            var procHandle = OpenProcess(desiredAccess, false, procId);
            if (procHandle == IntPtr.Zero)
            {
                return ApplyAffinityResult.Fail("OpenProcess failed: " + GetLastErrorMessage());
            }

            IntPtr codePtr = IntPtr.Zero;
            IntPtr thread = IntPtr.Zero;

            try
            {
                if (!TryResolveRemoteSetWindowDisplayAffinity(procHandle, out UInt64 remoteFunctionAddress, out Boolean is32Bit, out String errorMessage))
                {
                    return ApplyAffinityResult.Fail(errorMessage);
                }

                var asm = BuildRemoteCallStub(hWnd, dwAffinity, remoteFunctionAddress, is32Bit);
                codePtr = VirtualAllocEx(procHandle, IntPtr.Zero, asm.Count, 0x1000, 0x40);
                if (codePtr == IntPtr.Zero)
                {
                    return ApplyAffinityResult.Fail("VirtualAllocEx failed: " + GetLastErrorMessage());
                }

                if (!WriteProcessMemory(procHandle, codePtr, asm.ToArray(), asm.Count, out Int32 bytesWritten) || bytesWritten != asm.Count)
                {
                    return ApplyAffinityResult.Fail("WriteProcessMemory failed: " + GetLastErrorMessage());
                }

                thread = CreateRemoteThread(procHandle, IntPtr.Zero, 0, codePtr, IntPtr.Zero, 0, IntPtr.Zero);
                if (thread == IntPtr.Zero)
                {
                    return ApplyAffinityResult.Fail("CreateRemoteThread failed: " + GetLastErrorMessage());
                }

                var waitResult = WaitForSingleObject(thread, REMOTE_THREAD_TIMEOUT_MS);
                if (waitResult == 0xFFFFFFFF)
                {
                    return ApplyAffinityResult.Fail("WaitForSingleObject failed: " + GetLastErrorMessage());
                }

                if (waitResult != 0)
                {
                    return ApplyAffinityResult.Fail("Timed out while waiting for the remote SetWindowDisplayAffinity call to finish.");
                }

                if (!GetExitCodeThread(thread, out UInt32 exitCode))
                {
                    return ApplyAffinityResult.Fail("GetExitCodeThread failed: " + GetLastErrorMessage());
                }

                if (exitCode == 0)
                {
                    return ApplyAffinityResult.Fail("SetWindowDisplayAffinity returned false in the target process.");
                }

                return ApplyAffinityResult.Ok(dwAffinity, 1);
            }
            finally
            {
                if (thread != IntPtr.Zero) CloseHandle(thread);
                if (codePtr != IntPtr.Zero) VirtualFreeEx(procHandle, codePtr, 0, 0x8000);
                CloseHandle(procHandle);
            }
        }

        private static WindowEntry BuildWindowEntry(IntPtr hWnd, IntPtr rootHandleOverride = default(IntPtr))
        {
            if (hWnd == IntPtr.Zero || !IsWindow(hWnd)) return null;

            var rootHandle = rootHandleOverride != IntPtr.Zero ? rootHandleOverride : GetAncestor(hWnd, GA_ROOT);
            if (rootHandle == IntPtr.Zero) rootHandle = hWnd;

            var titleLength = GetWindowTextLength(hWnd);
            var titleBuilder = new StringBuilder(Math.Max(titleLength + 1, 1));
            GetWindowText(hWnd, titleBuilder, titleBuilder.Capacity);

            var classBuilder = new StringBuilder(MAX_CLASS_NAME);
            GetClassName(hWnd, classBuilder, classBuilder.Capacity);

            GetWindowThreadProcessId(hWnd, out Int32 processId);
            var processName = GetProcessName(processId);
            var className = classBuilder.ToString();
            var title = titleBuilder.ToString();
            var isVisible = IsWindowVisible(hWnd);
            var isCloaked = false;
            var isTopLevel = rootHandle == hWnd;
            var rootClassName = className;
            var rootProcessName = processName;

            if (DwmGetWindowAttribute(hWnd, DWMWA_CLOAKED, out Int32 cloakedValue, 4) == 0)
            {
                isCloaked = cloakedValue > 0;
            }

            if (!isTopLevel)
            {
                rootClassName = GetWindowClassName(rootHandle);
                GetWindowThreadProcessId(rootHandle, out Int32 rootProcessId);
                rootProcessName = GetProcessName(rootProcessId);
            }

            var isWeTypeCandidate = IsWeTypeCandidateWindow(processName, className, title)
                || (!isTopLevel && (IsWeTypeCandidateWindow(rootProcessName, rootClassName, title)
                    || MatchesWeTypeCandidateName(rootClassName)));
            var isSystemWindow = isWeTypeCandidate
                || IsKnownSystemWindow(processName, className, isTopLevel)
                || (!isTopLevel && IsKnownSystemWindow(rootProcessName, rootClassName, true) && IsSystemCandidateChildWindow(processName, className));

            return new WindowEntry
            {
                Handle = hWnd,
                RootHandle = rootHandle,
                Title = title,
                ClassName = className,
                ProcessName = processName,
                RootClassName = rootClassName,
                RootProcessName = rootProcessName,
                RuleKey = isWeTypeCandidate ? WETYPE_CANDIDATE_NAME : CreateRuleKey(processName, className, rootProcessName, rootClassName),
                IsVisible = isVisible,
                IsCloaked = isCloaked,
                IsTopLevel = isTopLevel,
                IsImeCandidate = isSystemWindow,
                IsSystemWindow = isSystemWindow,
                IsWeTypeCandidate = isWeTypeCandidate,
                IsHiddenFromList = isWeTypeCandidate
            };
        }

        private static Boolean ShouldIncludeWindow(WindowEntry entry, Boolean includeSystemCandidates, Boolean includeWeTypeCandidate)
        {
            if (entry.IsWeTypeCandidate)
            {
                return includeWeTypeCandidate;
            }

            if (entry.IsSystemWindow)
            {
                return includeSystemCandidates && IsSystemCandidateWindow(entry.ProcessName, entry.ClassName, entry.IsTopLevel);
            }

            return entry.IsVisible && !entry.IsCloaked && !String.IsNullOrWhiteSpace(entry.Title);
        }

        private static Boolean IsKnownSystemWindow(String processName, String className, Boolean isTopLevel)
        {
            return IsSystemCandidateWindow(processName, className, isTopLevel);
        }

        private static Boolean IsSystemCandidateWindow(String processName, String className, Boolean isTopLevel)
        {
            if (MatchesWeTypeCandidateName(processName) || MatchesWeTypeCandidateName(className)) return true;
            if (String.Equals(processName, "TextInputHost", StringComparison.OrdinalIgnoreCase) && isTopLevel) return true;
            if (String.Equals(processName, "TabTip", StringComparison.OrdinalIgnoreCase) && isTopLevel) return true;
            if (String.Equals(processName, "ctfmon", StringComparison.OrdinalIgnoreCase) && ImeClassNames.Contains(className)) return true;
            if (String.Equals(processName, "ApplicationFrameHost", StringComparison.OrdinalIgnoreCase) && HostClassNames.Contains(className)) return true;
            if (String.Equals(processName, "ApplicationFrameHost", StringComparison.OrdinalIgnoreCase) && EmbeddedImeClassNames.Contains(className)) return true;
            if (String.Equals(processName, "explorer", StringComparison.OrdinalIgnoreCase) && (String.Equals(className, "EdgeUiInputTopWndClass", StringComparison.OrdinalIgnoreCase) || EmbeddedImeClassNames.Contains(className))) return true;

            if (SystemProcessNames.Contains(processName) && HostClassNames.Contains(className)) return true;
            if (SystemProcessNames.Contains(processName) && ImeClassNames.Contains(className)) return true;
            if (SystemProcessNames.Contains(processName) && EmbeddedImeClassNames.Contains(className)) return true;

            return false;
        }

        private static Boolean IsSystemCandidateChildWindow(String processName, String className)
        {
            if (String.IsNullOrWhiteSpace(className)) return false;
            if (MatchesWeTypeCandidateName(processName) || MatchesWeTypeCandidateName(className)) return true;
            if (ImeClassNames.Contains(className)) return true;
            if (HostClassNames.Contains(className)) return true;
            if (EmbeddedImeClassNames.Contains(className)) return true;
            if (String.Equals(processName, "explorer", StringComparison.OrdinalIgnoreCase) && String.Equals(className, "EdgeUiInputTopWndClass", StringComparison.OrdinalIgnoreCase)) return true;
            return false;
        }

        private static Boolean ShouldScanChildWindows(WindowEntry entry, Boolean includeSystemCandidates, Boolean includeWeTypeCandidate)
        {
            if (!entry.IsTopLevel) return false;
            if (entry.IsSystemWindow) return true;
            if (includeSystemCandidates && SystemProcessNames.Contains(entry.ProcessName)) return true;
            if (includeWeTypeCandidate && (entry.IsVisible || !entry.IsCloaked)) return true;
            return false;
        }

        private static Boolean IsWeTypeCandidateWindow(String processName, String className, String title)
        {
            return MatchesWeTypeCandidateName(processName)
                || MatchesWeTypeCandidateName(className)
                || MatchesWeTypeCandidateName(title);
        }

        private static Boolean MatchesWeTypeCandidateName(String value)
        {
            return String.Equals(value, WETYPE_CANDIDATE_NAME, StringComparison.OrdinalIgnoreCase);
        }

        private static void AddWindowEntry(Dictionary<IntPtr, WindowEntry> windows, WindowEntry entry)
        {
            if (entry.Title == "Program Manager") entry.Title = "Desktop and Icons";
            windows[entry.Handle] = entry;
        }

        private static String GetWindowClassName(IntPtr hWnd)
        {
            var classBuilder = new StringBuilder(MAX_CLASS_NAME);
            GetClassName(hWnd, classBuilder, classBuilder.Capacity);
            return classBuilder.ToString();
        }

        private static String CreateRuleKey(String processName, String className, String rootProcessName, String rootClassName)
        {
            return $"{rootProcessName}|{rootClassName}|{processName}|{className}";
        }

        private static ApplyAffinityResult ApplySystemWindowProtection(WindowEntry entry, Int32 dwAffinity)
        {
            var rootHandle = entry.RootHandle != IntPtr.Zero ? entry.RootHandle : GetAncestor(entry.Handle, GA_ROOT);
            if (rootHandle == IntPtr.Zero) rootHandle = entry.Handle;

            var errorMessages = new List<String>();
            foreach (var affinityHandle in EnumerateAffinityTargets(entry, rootHandle))
            {
                var affinityResult = SetWindowDisplayAffinity(affinityHandle, dwAffinity);
                if (affinityResult.Success)
                {
                    affinityResult.CurrentAffinity = GetWindowDisplayAffinityValue(affinityHandle);
                    affinityResult.AffectedWindowCount = 1;
                    return affinityResult;
                }

                var targetKind = affinityHandle == entry.Handle ? "window" : "host";
                errorMessages.Add($"{targetKind} {FormatHandle(affinityHandle)} affinity: {affinityResult.ErrorMessage}");
            }

            var probeHandle = GetAffinityProbeHandle(entry);
            var currentAffinity = probeHandle != IntPtr.Zero
                ? GetWindowDisplayAffinityValue(probeHandle)
                : WDA_NONE;

            if (errorMessages.Count == 0)
            {
                return ApplyAffinityResult.Fail("No capture-only protection target could be resolved for this system window.", currentAffinity);
            }

            return ApplyAffinityResult.Fail(String.Join(" ", errorMessages.ToArray()), currentAffinity);
        }

        private static IEnumerable<IntPtr> EnumerateAffinityTargets(WindowEntry entry, IntPtr rootHandle = default(IntPtr))
        {
            if (entry == null) yield break;

            if (rootHandle == IntPtr.Zero)
            {
                rootHandle = entry.RootHandle != IntPtr.Zero ? entry.RootHandle : GetAncestor(entry.Handle, GA_ROOT);
            }

            if (entry.Handle != IntPtr.Zero && IsWindow(entry.Handle))
            {
                yield return entry.Handle;
            }

            if (entry.IsTopLevel) yield break;
            if (rootHandle == IntPtr.Zero || !IsWindow(rootHandle)) yield break;
            if (GenericContainerRootClassNames.Contains(entry.RootClassName)) yield break;
            if (!IsSystemCandidateWindow(entry.RootProcessName, entry.RootClassName, true)) yield break;
            if (rootHandle == entry.Handle) yield break;

            yield return rootHandle;
        }

        private static String FormatHandle(IntPtr hWnd)
        {
            return "0x" + hWnd.ToInt64().ToString("X");
        }

        private static String GetProcessName(Int32 processId)
        {
            if (processId <= 0) return String.Empty;

            try
            {
                using (var process = Process.GetProcessById(processId))
                {
                    return process.ProcessName;
                }
            }
            catch
            {
                return String.Empty;
            }
        }

        private static Boolean TryResolveRemoteSetWindowDisplayAffinity(IntPtr procHandle, out UInt64 address, out Boolean is32Bit, out String errorMessage)
        {
            address = 0;
            errorMessage = String.Empty;

            if (!IsWow64Process(procHandle, out is32Bit))
            {
                is32Bit = IntPtr.Size == 4;
            }

            if (!EnumProcessModulesEx(procHandle, new IntPtr[0], 0, out UInt32 bytesNeeded, 3))
            {
                errorMessage = "EnumProcessModulesEx failed while probing the target process: " + GetLastErrorMessage();
                return false;
            }

            var moduleCount = bytesNeeded / (UInt32)IntPtr.Size;
            if (moduleCount == 0)
            {
                errorMessage = "The target process does not expose any modules for SetWindowDisplayAffinity resolution.";
                return false;
            }

            var modules = new IntPtr[moduleCount];
            if (!EnumProcessModulesEx(procHandle, modules, bytesNeeded, out _, 3))
            {
                errorMessage = "EnumProcessModulesEx failed while reading target modules: " + GetLastErrorMessage();
                return false;
            }

            for (var i = 0; i < moduleCount && address == 0; i++)
            {
                var path = new StringBuilder(260);
                GetModuleFileNameEx(procHandle, modules[i], path, 260);

                if (!path.ToString().ToLowerInvariant().Contains("user32.dll")) continue;
                if (!GetModuleInformation(procHandle, modules[i], out MODULEINFO info, (uint)Marshal.SizeOf(typeof(MODULEINFO))))
                {
                    errorMessage = "GetModuleInformation failed for user32.dll: " + GetLastErrorMessage();
                    return false;
                }

                try
                {
                    var e_lfanew = ReadInt32(procHandle, (UInt64)info.lpBaseOfDll + 0x3C);
                    var ntHeaders = info.lpBaseOfDll + e_lfanew;
                    var optionalHeader = ntHeaders + 0x18;
                    var dataDirectory = optionalHeader + (is32Bit ? 0x60 : 0x70);
                    var exportDirectory = info.lpBaseOfDll + ReadInt32(procHandle, (UInt64)dataDirectory);
                    var names = info.lpBaseOfDll + ReadInt32(procHandle, (UInt64)exportDirectory + 0x20);
                    var ordinals = info.lpBaseOfDll + ReadInt32(procHandle, (UInt64)exportDirectory + 0x24);
                    var functions = info.lpBaseOfDll + ReadInt32(procHandle, (UInt64)exportDirectory + 0x1C);
                    var numFuncs = ReadInt32(procHandle, (UInt64)exportDirectory + 0x18);

                    for (var j = 0u; j < numFuncs && address == 0; j++)
                    {
                        var offset = (UInt64)ReadInt32(procHandle, (UInt64)names + j * 4);
                        var name = Encoding.UTF8.GetString(ReadBytes(procHandle, (UInt64)info.lpBaseOfDll + offset, 32));
                        if (!name.StartsWith("SetWindowDisplayAffinity")) continue;

                        var ordinal = (UInt64)(ReadInt32(procHandle, (UInt64)ordinals + j * 2) & 0xFFFF);
                        address = (UInt64)info.lpBaseOfDll + (UInt64)ReadInt32(procHandle, (UInt64)functions + ordinal * 4);
                    }
                }
                catch (InvalidOperationException ex)
                {
                    errorMessage = ex.Message;
                    return false;
                }
            }

            if (address == 0)
            {
                errorMessage = "Failed to locate SetWindowDisplayAffinity inside the target process.";
                return false;
            }

            return true;
        }

        private static List<Byte> BuildRemoteCallStub(IntPtr hWnd, Int32 dwAffinity, UInt64 functionAddress, Boolean is32Bit)
        {
            var asm = new List<Byte>();
            if (is32Bit)
            {
                asm.Add(0x68);
                asm.AddRange(BitConverter.GetBytes((UInt32)dwAffinity));
                asm.Add(0x68);
                asm.AddRange(BitConverter.GetBytes((UInt32)hWnd));
                asm.Add(0xB8);
                asm.AddRange(BitConverter.GetBytes((UInt32)functionAddress));
                asm.AddRange(new Byte[] { 0xFF, 0xD0 });
            }
            else
            {
                asm.AddRange(new Byte[] { 0x48, 0x83, 0xEC, 0x30 });
                asm.AddRange(new Byte[] { 0x48, 0xB9 });
                asm.AddRange(BitConverter.GetBytes((UInt64)hWnd));
                asm.AddRange(new Byte[] { 0x48, 0xBA });
                asm.AddRange(BitConverter.GetBytes((UInt64)dwAffinity));
                asm.AddRange(new Byte[] { 0x48, 0xB8 });
                asm.AddRange(BitConverter.GetBytes(functionAddress));
                asm.AddRange(new Byte[] { 0xFF, 0xD0 });
                asm.AddRange(new Byte[] { 0x48, 0x83, 0xC4, 0x30 });
            }

            asm.Add(0xC3);
            return asm;
        }

        private static Int32 ReadInt32(IntPtr procHandle, UInt64 addr)
        {
            var buffer = ReadBytes(procHandle, addr, 4);
            return BitConverter.ToInt32(buffer, 0);
        }

        private static Byte[] ReadBytes(IntPtr procHandle, UInt64 addr, Int32 size)
        {
            var buffer = new Byte[size];
            if (!ReadProcessMemory(procHandle, addr, buffer, size, out Int32 bytesRead) || bytesRead < size)
            {
                throw new InvalidOperationException("ReadProcessMemory failed while parsing target modules: " + GetLastErrorMessage());
            }

            return buffer;
        }

        private static String GetLastErrorMessage()
        {
            return new Win32Exception(Marshal.GetLastWin32Error()).Message;
        }
    }
}
