#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader 端到端黑盒测试核心驱动库 (E2EHarness.psm1)
.DESCRIPTION
    提供零第三方依赖的 Windows 自动化测试能力：
    1. 进程生命周期管理与 Win32 KillOnClose Job Object 防游离；
    2. UIA 韧性窗口/控件定位与 Win32 消息双模驱动（支持无前景桌面环境）；
    3. Per-Monitor DPI 感知与 PrintWindow 像素亮度/截图采样；
    4. HKCU 三分区零污染注册表沙盒与崩溃兜底恢复清单；
    5. 字节级 UTF-8 BOM/EOL 校验、跨进程 WM_DROPFILES、进程树内存采样与测试断言报告。
#>

Set-StrictMode -Version Latest

# 在任何 GUI/GDI 对象创建之前加载程序集并声明 DPI 感知
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$nativeCode = @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class E2ENative {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left, Top, Right, Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT {
        public int X, Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT {
        public uint type;
        public INPUTUNION U;
    }

    [StructLayout(LayoutKind.Explicit)]
    public struct INPUTUNION {
        [FieldOffset(0)] public MOUSEINPUT mi;
        [FieldOffset(0)] public KEYBDINPUT ki;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT {
        public int dx, dy;
        public uint mouseData, dwFlags, time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct KEYBDINPUT {
        public ushort wVk, wScan;
        public uint dwFlags, time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct DROPFILES {
        public uint pFiles;
        public int X;
        public int Y;
        public int fNC;
        public int fWide;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct IO_COUNTERS {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong OtherOperationCount;
        public ulong ReadTransferCount;
        public ulong WriteTransferCount;
        public ulong OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    public const uint INPUT_MOUSE = 0;
    public const uint INPUT_KEYBOARD = 1;
    public const uint KEYEVENTF_EXTENDEDKEY = 0x0001;
    public const uint KEYEVENTF_KEYUP = 0x0002;
    public const uint KEYEVENTF_UNICODE = 0x0004;
    public const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
    public const uint MOUSEEVENTF_LEFTUP = 0x0004;
    public const uint MOUSEEVENTF_WHEEL = 0x0800;

    public const uint WM_CLOSE = 0x0010;
    public const uint WM_GETTEXT = 0x000D;
    public const uint WM_GETTEXTLENGTH = 0x000E;
    public const uint WM_KEYDOWN = 0x0100;
    public const uint WM_KEYUP = 0x0101;
    public const uint WM_CHAR = 0x0102;
    public const uint EM_SETSEL = 0x00B1;
    public const uint EM_REPLACESEL = 0x00C2;
    public const uint BM_CLICK = 0x00F5;
    public const uint WM_MOUSEMOVE = 0x0200;
    public const uint WM_LBUTTONDOWN = 0x0201;
    public const uint WM_LBUTTONUP = 0x0202;
    public const uint WM_MOUSEWHEEL = 0x020A;
    public const uint WM_DROPFILES = 0x0233;

    public const int GWL_EXSTYLE = -20;
    public const long WS_EX_ACCEPTFILES = 0x00000010;
    public const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
    public const int JobObjectExtendedLimitInformation = 9;
    public const uint GHND = 0x0042;
    public const uint CF_UNICODETEXT = 13;
    public const uint PW_RENDERFULLCONTENT = 2;

    [DllImport("user32.dll")]
    public static extern bool SetProcessDPIAware();

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    public static extern bool ScreenToClient(IntPtr hWnd, ref POINT lpPoint);

    [DllImport("user32.dll")]
    public static extern bool ClientToScreen(IntPtr hWnd, ref POINT lpPoint);

    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)]
    private static extern IntPtr GetWindowLongPtr64(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongW", SetLastError = true)]
    private static extern int GetWindowLong32(IntPtr hWnd, int nIndex);

    public static long GetWindowExStyle(IntPtr hWnd) {
        if (IntPtr.Size == 8) {
            return GetWindowLongPtr64(hWnd, GWL_EXSTYLE).ToInt64();
        }
        return GetWindowLong32(hWnd, GWL_EXSTYLE);
    }

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool BringWindowToTop(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool AllowSetForegroundWindow(int dwProcessId);

    [DllImport("user32.dll")]
    public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);

    [DllImport("user32.dll")]
    public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);

    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int X, int Y);

    [DllImport("user32.dll")]
    public static extern IntPtr SetFocus(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);

    [DllImport("kernel32.dll")]
    public static extern uint GetCurrentThreadId();

    [DllImport("user32.dll")]
    public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);

    [DllImport("user32.dll")]
    public static extern bool GetKeyboardState(byte[] lpKeyState);

    [DllImport("user32.dll")]
    public static extern bool SetKeyboardState(byte[] lpKeyState);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, string lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int SendMessage(IntPtr hWnd, uint Msg, int wParam, StringBuilder lParam);

    [DllImport("user32.dll")]
    public static extern bool OpenClipboard(IntPtr hWndNewOwner);

    [DllImport("user32.dll")]
    public static extern bool CloseClipboard();

    [DllImport("user32.dll")]
    public static extern bool EmptyClipboard();

    [DllImport("user32.dll")]
    public static extern IntPtr SetClipboardData(uint uFormat, IntPtr hMem);

    [DllImport("user32.dll")]
    public static extern IntPtr GetClipboardData(uint uFormat);

    [DllImport("user32.dll")]
    public static extern bool IsClipboardFormatAvailable(uint format);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr GlobalAlloc(uint uFlags, UIntPtr dwBytes);

    [DllImport("kernel32.dll")]
    public static extern IntPtr GlobalFree(IntPtr hMem);

    [DllImport("kernel32.dll")]
    public static extern IntPtr GlobalLock(IntPtr hMem);

    [DllImport("kernel32.dll")]
    public static extern bool GlobalUnlock(IntPtr hMem);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr CreateJobObject(IntPtr lpJobAttributes, string lpName);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool SetInformationJobObject(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr hObject);

    public static IntPtr CreateKillOnCloseJob() {
        IntPtr hJob = CreateJobObject(IntPtr.Zero, null);
        if (hJob == IntPtr.Zero) return IntPtr.Zero;
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION info = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        int sz = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
        IntPtr ptr = Marshal.AllocHGlobal(sz);
        try {
            Marshal.StructureToPtr(info, ptr, false);
            if (!SetInformationJobObject(hJob, JobObjectExtendedLimitInformation, ptr, (uint)sz)) {
                CloseHandle(hJob);
                return IntPtr.Zero;
            }
            return hJob;
        } finally {
            Marshal.FreeHGlobal(ptr);
        }
    }

    public static bool SetClipboardUnicode(string text) {
        if (text == null) text = "";
        byte[] bytes = Encoding.Unicode.GetBytes(text + "\0");
        for (int i = 0; i < 8; i++) {
            if (OpenClipboard(IntPtr.Zero)) {
                try {
                    EmptyClipboard();
                    IntPtr hMem = GlobalAlloc(GHND, (UIntPtr)bytes.Length);
                    if (hMem == IntPtr.Zero) return false;
                    IntPtr ptr = GlobalLock(hMem);
                    if (ptr == IntPtr.Zero) {
                        GlobalFree(hMem);
                        return false;
                    }
                    try {
                        Marshal.Copy(bytes, 0, ptr, bytes.Length);
                    } finally {
                        GlobalUnlock(hMem);
                    }
                    if (SetClipboardData(CF_UNICODETEXT, hMem) == IntPtr.Zero) {
                        GlobalFree(hMem);
                        return false;
                    }
                    return true;
                } finally {
                    CloseClipboard();
                }
            }
            Thread.Sleep(30);
        }
        return false;
    }

    public static string GetClipboardUnicode() {
        for (int i = 0; i < 8; i++) {
            if (OpenClipboard(IntPtr.Zero)) {
                try {
                    if (!IsClipboardFormatAvailable(CF_UNICODETEXT)) return null;
                    IntPtr hData = GetClipboardData(CF_UNICODETEXT);
                    if (hData == IntPtr.Zero) return null;
                    IntPtr ptr = GlobalLock(hData);
                    if (ptr == IntPtr.Zero) return null;
                    try {
                        return Marshal.PtrToStringUni(ptr);
                    } finally {
                        GlobalUnlock(hData);
                    }
                } finally {
                    CloseClipboard();
                }
            }
            Thread.Sleep(30);
        }
        return null;
    }

    public static bool TryForceForeground(IntPtr hWnd) {
        if (hWnd == IntPtr.Zero) return false;
        if (IsIconic(hWnd)) ShowWindow(hWnd, 9);
        AllowSetForegroundWindow(-1);
        if (SetForegroundWindow(hWnd) && GetForegroundWindow() == hWnd) return true;

        IntPtr fg = GetForegroundWindow();
        uint dummy;
        uint fgTid = fg != IntPtr.Zero ? GetWindowThreadProcessId(fg, out dummy) : 0;
        uint curTid = GetCurrentThreadId();
        uint targetTid = GetWindowThreadProcessId(hWnd, out dummy);

        keybd_event(0x12, 0, 0, UIntPtr.Zero);
        keybd_event(0x12, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);

        bool aFg = false, aTarget = false;
        try {
            if (fgTid != 0 && fgTid != curTid) aFg = AttachThreadInput(curTid, fgTid, true);
            if (targetTid != 0 && targetTid != curTid) aTarget = AttachThreadInput(curTid, targetTid, true);
            BringWindowToTop(hWnd);
            ShowWindow(hWnd, 5);
            return SetForegroundWindow(hWnd) && GetForegroundWindow() == hWnd;
        } finally {
            if (aTarget) AttachThreadInput(curTid, targetTid, false);
            if (aFg) AttachThreadInput(curTid, fgTid, false);
        }
    }

    private static void SetFocusSafe(IntPtr hWnd) {
        uint dummy;
        uint tid = GetWindowThreadProcessId(hWnd, out dummy);
        uint cur = GetCurrentThreadId();
        bool att = (tid != 0 && tid != cur) && AttachThreadInput(cur, tid, true);
        try { SetFocus(hWnd); } finally { if (att) AttachThreadInput(cur, tid, false); }
    }

    private static INPUT MakeKey(ushort vk, bool extended, bool keyUp) {
        uint f = (extended ? KEYEVENTF_EXTENDEDKEY : 0) | (keyUp ? KEYEVENTF_KEYUP : 0);
        return new INPUT { type = INPUT_KEYBOARD, U = new INPUTUNION { ki = new KEYBDINPUT { wVk = vk, dwFlags = f } } };
    }

    public static void SendHotkeySmart(IntPtr mainHwnd, IntPtr focusHwnd, bool ctrl, bool shift, bool alt, ushort vk, bool extended) {
        IntPtr target = focusHwnd != IntPtr.Zero ? focusHwnd : mainHwnd;
        if (TryForceForeground(mainHwnd)) {
            if (focusHwnd != IntPtr.Zero) SetFocusSafe(focusHwnd);
            var list = new List<INPUT>();
            if (ctrl) list.Add(MakeKey(0x11, false, false));
            if (shift) list.Add(MakeKey(0x10, false, false));
            if (alt) list.Add(MakeKey(0x12, false, false));
            list.Add(MakeKey(vk, extended, false));
            list.Add(MakeKey(vk, extended, true));
            if (alt) list.Add(MakeKey(0x12, false, true));
            if (shift) list.Add(MakeKey(0x10, false, true));
            if (ctrl) list.Add(MakeKey(0x11, false, true));
            SendInput((uint)list.Count, list.ToArray(), Marshal.SizeOf(typeof(INPUT)));
            Thread.Sleep(60);
            return;
        }

        // 无前景桌面安全回退：AttachThreadInput + SetKeyboardState + PostMessage
        uint dummy;
        uint targetTid = GetWindowThreadProcessId(target, out dummy);
        uint curTid = GetCurrentThreadId();
        bool attached = (targetTid != 0 && targetTid != curTid) && AttachThreadInput(curTid, targetTid, true);
        try {
            if (focusHwnd != IntPtr.Zero) SetFocus(focusHwnd);
            byte[] oldState = new byte[256];
            byte[] newState = new byte[256];
            GetKeyboardState(oldState);
            Array.Copy(oldState, newState, 256);
            if (ctrl) newState[0x11] = 0x80;
            if (shift) newState[0x10] = 0x80;
            if (alt) newState[0x12] = 0x80;
            SetKeyboardState(newState);
            PostMessage(target, WM_KEYDOWN, (IntPtr)vk, (IntPtr)1);
            Thread.Sleep(85);
            PostMessage(target, WM_KEYUP, (IntPtr)vk, unchecked((IntPtr)0xC0000001));
            Thread.Sleep(35);
            SetKeyboardState(oldState);
        } finally {
            if (attached) AttachThreadInput(curTid, targetTid, false);
        }
    }

    public static void SendUnicodeSmart(IntPtr mainHwnd, IntPtr editHwnd, string text, bool replaceAll, bool preserveUndo) {
        if (text == null) text = "";
        if (!preserveUndo && replaceAll) {
            SendMessage(editHwnd, EM_SETSEL, IntPtr.Zero, (IntPtr)(-1));
            SendMessage(editHwnd, EM_REPLACESEL, (IntPtr)1, text);
            return;
        }
        if (TryForceForeground(mainHwnd)) {
            SetFocusSafe(editHwnd);
            if (replaceAll) {
                SendHotkeySmart(mainHwnd, editHwnd, true, false, false, 0x41, false); // Ctrl+A
                Thread.Sleep(30);
            }
            var list = new List<INPUT>();
            foreach (char ch in text) {
                if (ch == '\r') continue;
                if (ch == '\n') {
                    list.Add(MakeKey(0x0D, false, false));
                    list.Add(MakeKey(0x0D, false, true));
                    continue;
                }
                list.Add(new INPUT { type = INPUT_KEYBOARD, U = new INPUTUNION { ki = new KEYBDINPUT { wScan = ch, dwFlags = KEYEVENTF_UNICODE } } });
                list.Add(new INPUT { type = INPUT_KEYBOARD, U = new INPUTUNION { ki = new KEYBDINPUT { wScan = ch, dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } } });
            }
            if (list.Count > 0) SendInput((uint)list.Count, list.ToArray(), Marshal.SizeOf(typeof(INPUT)));
            Thread.Sleep(50);
            return;
        }

        // 无前景桌面下确保保留撤销栈：EM_SETSEL + EM_REPLACESEL(fCanUndo=1) 或 WM_CHAR
        if (replaceAll) {
            SendMessage(editHwnd, EM_SETSEL, IntPtr.Zero, (IntPtr)(-1));
            SendMessage(editHwnd, EM_REPLACESEL, (IntPtr)1, text);
        } else {
            foreach (char ch in text) {
                SendMessage(editHwnd, WM_CHAR, (IntPtr)(int)ch, (IntPtr)1);
            }
        }
    }

    public static string ReadEditTextWmGetText(IntPtr hEdit) {
        int len = (int)SendMessage(hEdit, WM_GETTEXTLENGTH, IntPtr.Zero, IntPtr.Zero);
        if (len <= 0) return "";
        StringBuilder sb = new StringBuilder(len + 4);
        SendMessage(hEdit, WM_GETTEXT, sb.Capacity, sb);
        return sb.ToString();
    }

    public static bool ClickScreenPoint(IntPtr mainHwnd, int screenX, int screenY) {
        if (TryForceForeground(mainHwnd)) {
            SetCursorPos(screenX, screenY);
            Thread.Sleep(20);
            INPUT[] inputs = new INPUT[2];
            inputs[0] = new INPUT { type = INPUT_MOUSE, U = new INPUTUNION { mi = new MOUSEINPUT { dwFlags = MOUSEEVENTF_LEFTDOWN } } };
            inputs[1] = new INPUT { type = INPUT_MOUSE, U = new INPUTUNION { mi = new MOUSEINPUT { dwFlags = MOUSEEVENTF_LEFTUP } } };
            SendInput(2, inputs, Marshal.SizeOf(typeof(INPUT)));
            Thread.Sleep(40);
            return true;
        }
        POINT pt = new POINT { X = screenX, Y = screenY };
        ScreenToClient(mainHwnd, ref pt);
        IntPtr lParam = (IntPtr)((pt.Y << 16) | (pt.X & 0xFFFF));
        PostMessage(mainHwnd, WM_LBUTTONDOWN, (IntPtr)1, lParam);
        Thread.Sleep(30);
        PostMessage(mainHwnd, WM_LBUTTONUP, IntPtr.Zero, lParam);
        Thread.Sleep(30);
        return true;
    }

    public static void DragMouseClient(IntPtr hwnd, int x1, int y1, int x2, int y2) {
        POINT p1 = new POINT { X = x1, Y = y1 };
        POINT p2 = new POINT { X = x2, Y = y2 };
        ClientToScreen(hwnd, ref p1);
        ClientToScreen(hwnd, ref p2);
        if (TryForceForeground(hwnd)) {
            SetCursorPos(p1.X, p1.Y);
            Thread.Sleep(20);
            INPUT[] down = new INPUT[1];
            down[0] = new INPUT { type = INPUT_MOUSE, U = new INPUTUNION { mi = new MOUSEINPUT { dwFlags = MOUSEEVENTF_LEFTDOWN } } };
            SendInput(1, down, Marshal.SizeOf(typeof(INPUT)));
            for (int i = 1; i <= 5; i++) {
                int cx = p1.X + (p2.X - p1.X) * i / 5;
                int cy = p1.Y + (p2.Y - p1.Y) * i / 5;
                SetCursorPos(cx, cy);
                Thread.Sleep(15);
            }
            INPUT[] up = new INPUT[1];
            up[0] = new INPUT { type = INPUT_MOUSE, U = new INPUTUNION { mi = new MOUSEINPUT { dwFlags = MOUSEEVENTF_LEFTUP } } };
            SendInput(1, up, Marshal.SizeOf(typeof(INPUT)));
            Thread.Sleep(40);
            return;
        }
        IntPtr lp1 = (IntPtr)((y1 << 16) | (x1 & 0xFFFF));
        IntPtr lp2 = (IntPtr)((y2 << 16) | (x2 & 0xFFFF));
        PostMessage(hwnd, WM_LBUTTONDOWN, (IntPtr)1, lp1);
        Thread.Sleep(25);
        PostMessage(hwnd, WM_MOUSEMOVE, (IntPtr)1, lp2);
        Thread.Sleep(25);
        PostMessage(hwnd, WM_LBUTTONUP, IntPtr.Zero, lp2);
        Thread.Sleep(30);
    }

    public static void SendMouseWheel(IntPtr hwnd, int delta) {
        RECT r;
        GetWindowRect(hwnd, out r);
        int cx = (r.Left + r.Right) / 2;
        int cy = (r.Top + r.Bottom) / 2;
        if (TryForceForeground(hwnd)) {
            SetCursorPos(cx, cy);
            INPUT[] inp = new INPUT[1];
            inp[0] = new INPUT { type = INPUT_MOUSE, U = new INPUTUNION { mi = new MOUSEINPUT { mouseData = unchecked((uint)delta), dwFlags = MOUSEEVENTF_WHEEL } } };
            SendInput(1, inp, Marshal.SizeOf(typeof(INPUT)));
            Thread.Sleep(40);
            return;
        }
        IntPtr wParam = (IntPtr)(delta << 16);
        IntPtr lParam = (IntPtr)((cy << 16) | (cx & 0xFFFF));
        PostMessage(hwnd, WM_MOUSEWHEEL, wParam, lParam);
        Thread.Sleep(40);
    }

    public static bool PostDropFiles(IntPtr hwnd, string[] paths) {
        if (hwnd == IntPtr.Zero || paths == null || paths.Length == 0) return false;
        string joined = string.Join("\0", paths) + "\0\0";
        byte[] pathBytes = Encoding.Unicode.GetBytes(joined);
        int headerSize = Marshal.SizeOf(typeof(DROPFILES));
        IntPtr hDrop = GlobalAlloc(GHND, (UIntPtr)(headerSize + pathBytes.Length));
        if (hDrop == IntPtr.Zero) return false;

        IntPtr ptr = GlobalLock(hDrop);
        if (ptr == IntPtr.Zero) {
            GlobalFree(hDrop);
            return false;
        }
        try {
            DROPFILES df = new DROPFILES {
                pFiles = (uint)headerSize,
                X = 20,
                Y = 20,
                fNC = 0,
                fWide = 1
            };
            Marshal.StructureToPtr(df, ptr, false);
            Marshal.Copy(pathBytes, 0, new IntPtr(ptr.ToInt64() + headerSize), pathBytes.Length);
        } finally {
            GlobalUnlock(hDrop);
        }

        bool ok = PostMessage(hwnd, WM_DROPFILES, hDrop, IntPtr.Zero);
        if (!ok) {
            GlobalFree(hDrop);
        } else {
            Thread.Sleep(80);
        }
        return ok;
    }

    public static Bitmap CaptureWindowBitmap(IntPtr hWnd) {
        RECT wr;
        if (!GetWindowRect(hWnd, out wr)) return null;
        int w = wr.Right - wr.Left;
        int h = wr.Bottom - wr.Top;
        if (w <= 0 || h <= 0) return null;
        Bitmap bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(bmp)) {
            IntPtr hdc = g.GetHdc();
            bool ok = PrintWindow(hWnd, hdc, PW_RENDERFULLCONTENT);
            if (!ok) {
                ok = PrintWindow(hWnd, hdc, 0);
            }
            g.ReleaseHdc(hdc);
            if (!ok) {
                bmp.Dispose();
                return null;
            }
        }
        return bmp;
    }

    public static double MeasureWindowCenterLuminance(IntPtr hWnd) {
        using (Bitmap bmp = CaptureWindowBitmap(hWnd)) {
            if (bmp == null) return -1;
            int w = bmp.Width, h = bmp.Height;
            double sum = 0;
            int cnt = 0;
            int x0 = (int)(w * 0.2), x1 = (int)(w * 0.8);
            int y0 = (int)(h * 0.25), y1 = (int)(h * 0.85);
            for (int y = y0; y < y1; y += 6) {
                for (int x = x0; x < x1; x += 6) {
                    Color c = bmp.GetPixel(x, y);
                    sum += 0.2126 * c.R + 0.7152 * c.G + 0.0722 * c.B;
                    cnt++;
                }
            }
            return cnt > 0 ? (sum / cnt) : -1;
        }
    }

    public static double ComputeBitmapDiffRatio(Bitmap a, Bitmap b) {
        if (a == null || b == null) return -1;
        int w = Math.Min(a.Width, b.Width);
        int h = Math.Min(a.Height, b.Height);
        if (w <= 0 || h <= 0) return -1;
        int x0 = (int)(w * 0.1), x1 = (int)(w * 0.9);
        int y0 = (int)(h * 0.18), y1 = (int)(h * 0.92);
        int total = 0, diff = 0;
        for (int y = y0; y < y1; y += 5) {
            for (int x = x0; x < x1; x += 5) {
                Color ca = a.GetPixel(x, y);
                Color cb = b.GetPixel(x, y);
                int d = Math.Abs(ca.R - cb.R) + Math.Abs(ca.G - cb.G) + Math.Abs(ca.B - cb.B);
                if (d > 15) diff++;
                total++;
            }
        }
        return total > 0 ? ((double)diff / total) : 0;
    }
}
'@

if (-not ([System.Management.Automation.PSTypeName]'E2ENative').Type) {
    Add-Type -TypeDefinition $nativeCode -ReferencedAssemblies @("System.Drawing.dll")
}
[void][E2ENative]::SetProcessDPIAware()

# 模块级状态：Job Object、PID 追踪、沙盒目录追踪
$script:GlobalJobHandle = [IntPtr]::Zero
$script:TrackedPids = New-Object 'System.Collections.Generic.HashSet[int]'
$script:TrackedTempDirs = New-Object 'System.Collections.Generic.HashSet[string]'
$script:IgnoredWindowClasses = @(
    "UAC_InputIndicatorOverlayWnd",
    "UAC Input Indicator",
    "IME",
    "MSCTFIME UI",
    "ConsoleWindowClass",
    "PseudoConsoleWindow",
    "CiceroUIWndFrame",
    "tooltips_class32"
)
$script:RegRecoveryManifestPath = Join-Path $env:TEMP "mdreader_e2e_reg_recovery.json"

function Initialize-E2EJobObject {
    [CmdletBinding()]
    param()
    if ($script:GlobalJobHandle -eq [IntPtr]::Zero) {
        $hJob = [E2ENative]::CreateKillOnCloseJob()
        if ($hJob -eq [IntPtr]::Zero) {
            throw "创建 KillOnClose Win32 Job Object 失败"
        }
        $script:GlobalJobHandle = $hJob
    }
    return $script:GlobalJobHandle
}

function Close-E2EJobObject {
    [CmdletBinding()]
    param()
    if ($script:GlobalJobHandle -ne [IntPtr]::Zero) {
        [void][E2ENative]::CloseHandle($script:GlobalJobHandle)
        $script:GlobalJobHandle = [IntPtr]::Zero
    }
}

function Format-CliArgumentsString {
    param([string[]]$Arguments)
    if (-not $Arguments -or $Arguments.Count -eq 0) { return "" }
    $escaped = foreach ($arg in $Arguments) {
        if ($null -eq $arg) { '""'; continue }
        if ($arg -eq "") { '""'; continue }
        if ($arg -match '[\s"]') {
            $s = $arg -replace '(\\*)"', '$1$1\"'
            $s = $s -replace '(\\+)$', '$1$1'
            '"' + $s + '"'
        } else {
            $arg
        }
    }
    return ($escaped -join ' ')
}

function Remove-ReadOnlyRecursive {
    param([string]$TargetDir)
    if (-not $TargetDir -or -not (Test-Path -LiteralPath $TargetDir)) { return }
    try {
        Get-ChildItem -LiteralPath $TargetDir -Recurse -Force -ErrorAction SilentlyContinue | ForEach-Object {
            if (($_.Attributes -band [System.IO.FileAttributes]::ReadOnly) -ne 0) {
                $_.Attributes = ($_.Attributes -band (-bnot [System.IO.FileAttributes]::ReadOnly))
            }
        }
        $rootItem = Get-Item -LiteralPath $TargetDir -Force -ErrorAction SilentlyContinue
        if ($rootItem -and (($rootItem.Attributes -band [System.IO.FileAttributes]::ReadOnly) -ne 0)) {
            $rootItem.Attributes = ($rootItem.Attributes -band (-bnot [System.IO.FileAttributes]::ReadOnly))
        }
        Remove-Item -LiteralPath $TargetDir -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        # 尽力清理
    }
}

function New-E2ECaseContext {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$CaseId,
        [Parameter(Mandatory = $true)][string]$ExePath,
        [switch]$CopyExeToSandbox,
        [string]$RunId = ""
    )
    $resolvedExe = (Resolve-Path -LiteralPath $ExePath).Path
    if (-not $RunId) {
        $RunId = [Guid]::NewGuid().ToString("N").Substring(0, 8)
    }
    $safeCase = ($CaseId -replace '[^a-zA-Z0-9_\-\.]', '_')
    $rootTemp = Join-Path $env:TEMP ("mdreader_e2e_" + $RunId)
    $sandboxDir = Join-Path $rootTemp $safeCase
    New-Item -ItemType Directory -Force -Path $sandboxDir | Out-Null
    [void]$script:TrackedTempDirs.Add($rootTemp)

    $effectiveExe = $resolvedExe
    if ($CopyExeToSandbox) {
        $exeName = Split-Path -Leaf $resolvedExe
        $effectiveExe = Join-Path $sandboxDir $exeName
        Copy-Item -LiteralPath $resolvedExe -Destination $effectiveExe -Force
    }

    return @{
        CaseId          = $CaseId
        RunId           = $RunId
        RootTempDir     = $rootTemp
        SandboxDir      = $sandboxDir
        SourceExePath   = $resolvedExe
        ExePath         = $effectiveExe
        CopiedToSandbox = [bool]$CopyExeToSandbox
    }
}

function Remove-E2ECaseContext {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][hashtable]$Context
    )
    if ($Context.SandboxDir -and (Test-Path -LiteralPath $Context.SandboxDir)) {
        Remove-ReadOnlyRecursive -TargetDir $Context.SandboxDir
    }
    if ($Context.RootTempDir -and (Test-Path -LiteralPath $Context.RootTempDir)) {
        $remaining = Get-ChildItem -LiteralPath $Context.RootTempDir -Force -ErrorAction SilentlyContinue
        if (-not $remaining) {
            Remove-ReadOnlyRecursive -TargetDir $Context.RootTempDir
            [void]$script:TrackedTempDirs.Remove($Context.RootTempDir)
        }
    }
}

function Get-DescendantProcessIds {
    param([int]$RootPid)
    $result = New-Object 'System.Collections.Generic.List[int]'
    $allProcs = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId, ParentProcessId, Name
    if (-not $allProcs) { return $result }

    $childrenMap = @{}
    foreach ($p in $allProcs) {
        $ppid = [int]$p.ParentProcessId
        if (-not $childrenMap.ContainsKey($ppid)) {
            $childrenMap[$ppid] = New-Object 'System.Collections.Generic.List[int]'
        }
        [void]$childrenMap[$ppid].Add([int]$p.ProcessId)
    }

    # 后序遍历，保证先杀子孙进程再杀父进程
    $visit = $null
    $visit = {
        param([int]$curPid)
        if ($childrenMap.ContainsKey($curPid)) {
            foreach ($cid in $childrenMap[$curPid]) {
                & $visit $cid
                [void]$result.Add($cid)
            }
        }
    }
    & $visit $RootPid
    return $result
}

function Stop-ProcessTreeById {
    param([int]$ProcessId)
    if ($ProcessId -le 0) { return }
    $descendants = Get-DescendantProcessIds -RootPid $ProcessId
    foreach ($cid in $descendants) {
        try { Stop-Process -Id $cid -Force -ErrorAction SilentlyContinue } catch {}
    }
    try { Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue } catch {}
    [void]$script:TrackedPids.Remove($ProcessId)
}

function Start-AppInstance {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][hashtable]$Context,
        [string[]]$Arguments = @(),
        [int]$TimeoutMs = 15000,
        [switch]$NoWaitWindow,
        [string]$TitleRegex = "MDReader"
    )
    $hJob = Initialize-E2EJobObject
    $exePath = $Context.ExePath
    if (-not (Test-Path -LiteralPath $exePath)) {
        throw "可执行文件不存在: $exePath"
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exePath
    $psi.Arguments = Format-CliArgumentsString -Arguments $Arguments
    $psi.WorkingDirectory = $Context.SandboxDir
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $false

    $proc = [System.Diagnostics.Process]::Start($psi)
    if ($null -eq $proc) {
        throw "启动进程失败: $exePath"
    }

    [void]$script:TrackedPids.Add($proc.Id)
    try {
        [void][E2ENative]::AssignProcessToJobObject($hJob, $proc.Handle)
    } catch {
        # 忽略嵌套限制等非致命情况，仍有 TrackedPids 进程树清理兜底
    }

    $session = @{
        Process    = $proc
        ProcessId  = $proc.Id
        Context    = $Context
        MainWindow = $null
        Hwnd       = [IntPtr]::Zero
        Title      = ""
    }

    if (-not $NoWaitWindow) {
        try { [void]$proc.WaitForInputIdle(3000) } catch {}
        $win = Find-MainWindow -ProcessId $proc.Id -TimeoutMs $TimeoutMs -TitleRegex $TitleRegex
        $session.MainWindow = $win
        $session.Hwnd = [IntPtr]$win.Current.NativeWindowHandle
        $session.Title = $win.Current.Name
    }

    return $session
}

function Stop-AppInstance {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][hashtable]$Session,
        [int]$GracefulMs = 1500
    )
    $proc = $Session.Process
    $targetPid = [int]$Session.ProcessId
    if ($null -eq $proc) {
        Stop-ProcessTreeById -ProcessId $targetPid
        return
    }

    try {
        $proc.Refresh()
        if (-not $proc.HasExited) {
            if ($proc.MainWindowHandle -ne [IntPtr]::Zero) {
                [void]$proc.CloseMainWindow()
            } elseif ($Session.Hwnd -and $Session.Hwnd -ne [IntPtr]::Zero) {
                [void][E2ENative]::PostMessage($Session.Hwnd, [E2ENative]::WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
            }

            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            while ($sw.ElapsedMilliseconds -lt $GracefulMs) {
                $proc.Refresh()
                if ($proc.HasExited) { break }
                # 若关闭期间弹出未保存确认框，超时后将强制清理
                Start-Sleep -Milliseconds 60
            }
        }
    } catch {}

    try {
        $proc.Refresh()
        if (-not $proc.HasExited) {
            Stop-ProcessTreeById -ProcessId $targetPid
            try { [void]$proc.WaitForExit(2000) } catch {}
        }
    } catch {
        Stop-ProcessTreeById -ProcessId $targetPid
    }
    [void]$script:TrackedPids.Remove($targetPid)
}

function Invoke-AppCleanupAll {
    [CmdletBinding()]
    param()
    $pidsCopy = New-Object 'System.Collections.Generic.List[int]'
    foreach ($tp in $script:TrackedPids) { [void]$pidsCopy.Add($tp) }
    foreach ($p in $pidsCopy) {
        Stop-ProcessTreeById -ProcessId $p
    }
    Close-E2EJobObject

    $dirsCopy = New-Object 'System.Collections.Generic.List[string]'
    foreach ($td in $script:TrackedTempDirs) { [void]$dirsCopy.Add($td) }
    foreach ($d in $dirsCopy) {
        Remove-ReadOnlyRecursive -TargetDir $d
        [void]$script:TrackedTempDirs.Remove($d)
    }
}

function Get-ProcessTreeMetrics {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]$Process
    )
    $rootPid = if ($Process -is [int]) { $Process } else { [int]$Process.Id }
    $procObj = if ($Process -is [int]) { Get-Process -Id $rootPid -ErrorAction Stop } else { $Process }
    $procObj.Refresh()

    $allWmi = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId, ParentProcessId, Name)
    $childrenByParent = @{}
    $procNameMap = @{}
    foreach ($item in $allWmi) {
        $id = [int]$item.ProcessId
        $ppid = [int]$item.ParentProcessId
        $procNameMap[$id] = $item.Name
        if (-not $childrenByParent.ContainsKey($ppid)) {
            $childrenByParent[$ppid] = New-Object 'System.Collections.Generic.List[int]'
        }
        [void]$childrenByParent[$ppid].Add($id)
    }

    $treeIds = New-Object 'System.Collections.Generic.List[int]'
    [void]$treeIds.Add($rootPid)
    $queue = New-Object 'System.Collections.Generic.Queue[int]'
    $queue.Enqueue($rootPid)
    $childNames = New-Object 'System.Collections.Generic.List[string]'

    while ($queue.Count -gt 0) {
        $cur = $queue.Dequeue()
        if ($childrenByParent.ContainsKey($cur)) {
            foreach ($cid in $childrenByParent[$cur]) {
                [void]$treeIds.Add($cid)
                $queue.Enqueue($cid)
                if ($procNameMap.ContainsKey($cid)) {
                    [void]$childNames.Add([string]$procNameMap[$cid])
                }
            }
        }
    }

    [long]$totalWs = 0
    [long]$totalPrivate = 0
    [int]$totalHandles = 0
    [int]$aliveCount = 0

    foreach ($tid in $treeIds) {
        try {
            $gp = if ($tid -eq $rootPid) { $procObj } else { Get-Process -Id $tid -ErrorAction SilentlyContinue }
            if ($gp) {
                $gp.Refresh()
                $totalWs += $gp.WorkingSet64
                $totalPrivate += $gp.PrivateMemorySize64
                $totalHandles += $gp.HandleCount
                $aliveCount++
            }
        } catch {}
    }

    return @{
        RootPid         = $rootPid
        WorkingSetMB    = [math]::Round($totalWs / 1MB, 2)
        PrivateMemoryMB = [math]::Round($totalPrivate / 1MB, 2)
        ProcessCount    = $aliveCount
        HandleCount     = $totalHandles
        ChildNames      = $childNames.ToArray()
    }
}

function Find-MainWindow {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [int]$TimeoutMs = 15000,
        [string]$TitleRegex = "MDReader"
    )
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $ProcessId
    )
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try {
            $wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
            foreach ($w in $wins) {
                $cls = $w.Current.ClassName
                if ($cls -in $script:IgnoredWindowClasses) { continue }
                if ($cls -eq "#32770") { continue }
                if ($w.Current.BoundingRectangle.Width -lt 100 -or $w.Current.BoundingRectangle.Height -lt 80) { continue }
                $name = $w.Current.Name
                if (-not $TitleRegex -or ($name -match $TitleRegex)) {
                    return $w
                }
            }
        } catch {}
        Start-Sleep -Milliseconds 100
    }
    throw "等待进程 $ProcessId 主窗口超时 (${TimeoutMs}ms, TitleRegex='$TitleRegex')"
}

function Find-Control {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [string]$AutomationId = "",
        [string[]]$FallbackNames = @(),
        [string]$ClassRegex = "",
        [int]$MinHeight = 0,
        [int]$MaxHeight = 0,
        [switch]$ExcludeTitleBar,
        [int]$TimeoutMs = 5000
    )
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $sysNames = @("最小化", "最大化", "向下还原", "关闭", "Minimize", "Maximize", "Restore", "Close", "System")
    $sysIds = @("Minimize", "Maximize", "Close")

    do {
        try {
            if ($AutomationId) {
                $cond = New-Object System.Windows.Automation.PropertyCondition(
                    [System.Windows.Automation.AutomationElement]::AutomationIdProperty, $AutomationId
                )
                $hits = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $cond)
                foreach ($el in $hits) {
                    if ($el.Current.IsOffscreen) { continue }
                    $rect = $el.Current.BoundingRectangle
                    if ($MinHeight -gt 0 -and $rect.Height -lt $MinHeight) { continue }
                    if ($MaxHeight -gt 0 -and $rect.Height -gt $MaxHeight) { continue }
                    return $el
                }
            }

            $all = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
            foreach ($c in $all) {
                if ($c.Current.IsOffscreen) { continue }
                $aid = $c.Current.AutomationId
                $cname = $c.Current.Name
                $ccls = $c.Current.ClassName
                if ($ExcludeTitleBar -and (($aid -in $sysIds) -or ($cname -in $sysNames))) { continue }

                $rect = $c.Current.BoundingRectangle
                if ($rect.Width -le 0 -or $rect.Height -le 0) { continue }
                if ($MinHeight -gt 0 -and $rect.Height -lt $MinHeight) { continue }
                if ($MaxHeight -gt 0 -and $rect.Height -gt $MaxHeight) { continue }

                if ($ClassRegex -and ($ccls -notmatch $ClassRegex)) { continue }
                if ($FallbackNames.Count -gt 0) {
                    if ($cname -in $FallbackNames) { return $c }
                } elseif ($ClassRegex) {
                    return $c
                }
            }
        } catch {}
        if ($sw.ElapsedMilliseconds -ge $TimeoutMs) { break }
        Start-Sleep -Milliseconds 80
    } while ($true)

    return $null
}

function Get-ToolbarActionButtons {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root
    )
    $sysIds = @("Minimize", "Maximize", "Close")
    $sysNames = @("最小化", "最大化", "向下还原", "关闭", "Minimize", "Maximize", "Restore", "Close", "System")
    $winRect = $Root.Current.BoundingRectangle
    $titleBarCutoff = $winRect.Top + 34

    $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
    $all = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $buttons = New-Object 'System.Collections.Generic.List[System.Windows.Automation.AutomationElement]'

    foreach ($el in $all) {
        try {
            if ($el.Current.IsOffscreen) { continue }
            $ctrlType = $el.Current.ControlType
            $cls = $el.Current.ClassName
            $isBtn = ($ctrlType -eq [System.Windows.Automation.ControlType]::Button) -or ($cls -match 'Button|BUTTON')
            if (-not $isBtn) { continue }

            $aid = $el.Current.AutomationId
            $name = $el.Current.Name
            if ($aid -in $sysIds) { continue }

            # 检查父节点是否为 TitleBar
            $inTitleBar = $false
            try {
                $parent = $walker.GetParent($el)
                if ($parent -and ($parent.Current.ControlType -eq [System.Windows.Automation.ControlType]::TitleBar)) {
                    $inTitleBar = $true
                }
            } catch {}
            if ($inTitleBar) { continue }

            $r = $el.Current.BoundingRectangle
            if ($r.Width -le 0 -or $r.Height -le 0) { continue }
            if (($name -in $sysNames) -and ($r.Top -lt $titleBarCutoff)) { continue }

            [void]$buttons.Add($el)
        } catch {}
    }
    return $buttons.ToArray()
}

function Invoke-Button {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element,
        [IntPtr]$MainHwnd = [IntPtr]::Zero
    )
    # 1. 优先尝试 UIA InvokePattern
    $inv = $null
    try {
        if ($Element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$inv)) {
            $inv.Invoke()
            return
        }
    } catch {}

    # 2. 次选 Win32 异步 PostMessage(BM_CLICK)（严禁同步 SendMessage 避免模态弹窗阻塞）
    $h = [IntPtr]$Element.Current.NativeWindowHandle
    if ($h -ne [IntPtr]::Zero) {
        [void][E2ENative]::PostMessage($h, [E2ENative]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
        return
    }

    # 3. 兜底坐标点击
    $rect = $Element.Current.BoundingRectangle
    if ($rect.Width -gt 0 -and $rect.Height -gt 0) {
        $cx = [int]($rect.Left + $rect.Width / 2)
        $cy = [int]($rect.Top + $rect.Height / 2)
        $targetHwnd = if ($MainHwnd -ne [IntPtr]::Zero) { $MainHwnd } else { [IntPtr]::Zero }
        [void][E2ENative]::ClickScreenPoint($targetHwnd, $cx, $cy)
        return
    }

    throw "无法触发按钮: Name='$($Element.Current.Name)', AutomationId='$($Element.Current.AutomationId)'"
}

function Send-Hotkey {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][IntPtr]$MainHwnd,
        [IntPtr]$FocusHwnd = [IntPtr]::Zero,
        [switch]$Ctrl,
        [switch]$Shift,
        [switch]$Alt,
        [Parameter(Mandatory = $true)][uint16]$Vk,
        [switch]$Extended
    )
    [E2ENative]::SendHotkeySmart($MainHwnd, $FocusHwnd, [bool]$Ctrl, [bool]$Shift, [bool]$Alt, $Vk, [bool]$Extended)
}

function Get-EditorText {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$EditorElement
    )
    $tp = $null
    try {
        if ($EditorElement.TryGetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern, [ref]$tp)) {
            $txt = $tp.DocumentRange.GetText(-1)
            return $txt
        }
    } catch {}

    $h = [IntPtr]$EditorElement.Current.NativeWindowHandle
    if ($h -ne [IntPtr]::Zero) {
        return [E2ENative]::ReadEditTextWmGetText($h)
    }

    $vp = $null
    if ($EditorElement.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$vp)) {
        return $vp.Current.Value
    }

    throw "无法读取编辑区文本：未支持 TextPattern / NativeWindowHandle / ValuePattern"
}

function Set-EditorText {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][IntPtr]$MainHwnd,
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$EditorElement,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [switch]$ReplaceAll,
        [switch]$PreserveUndo
    )
    $editHwnd = [IntPtr]$EditorElement.Current.NativeWindowHandle
    if ($editHwnd -eq [IntPtr]::Zero) {
        throw "编辑区缺少有效的 NativeWindowHandle"
    }
    try { $EditorElement.SetFocus() } catch {}
    $keepUndo = if ($PSBoundParameters.ContainsKey('PreserveUndo')) { [bool]$PreserveUndo } else { $true }
    [E2ENative]::SendUnicodeSmart($MainHwnd, $editHwnd, $Text, [bool]$ReplaceAll, $keepUndo)
}

function Set-ClipboardTextWin32 {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text
    )
    $ok = [E2ENative]::SetClipboardUnicode($Text)
    if (-not $ok) {
        throw "Win32 OpenClipboard/SetClipboardData 写入失败"
    }
}

function Get-ClipboardTextWin32 {
    [CmdletBinding()]
    param()
    return [E2ENative]::GetClipboardUnicode()
}

function Get-WindowLuminance {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Hwnd
    )
    return [E2ENative]::MeasureWindowCenterLuminance($Hwnd)
}

function Capture-WindowPng {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Hwnd,
        [Parameter(Mandatory = $true)][string]$OutPath
    )
    $parent = Split-Path -Parent $OutPath
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    $bmp = [E2ENative]::CaptureWindowBitmap($Hwnd)
    if ($null -eq $bmp) {
        throw "PrintWindow 捕获窗口图像失败 (Hwnd=$Hwnd)"
    }
    try {
        $bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $bmp.Dispose()
    }
    return $OutPath
}

function Compare-WindowCaptureDiff {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$PngPathA,
        [Parameter(Mandatory = $true)][string]$PngPathB
    )
    $bmpA = [System.Drawing.Bitmap]::FromFile($PngPathA)
    $bmpB = [System.Drawing.Bitmap]::FromFile($PngPathB)
    try {
        return [E2ENative]::ComputeBitmapDiffRatio($bmpA, $bmpB)
    } finally {
        $bmpA.Dispose()
        $bmpB.Dispose()
    }
}

function Send-DropFiles {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Hwnd,
        [Parameter(Mandatory = $true)][string[]]$Paths
    )
    $ok = [E2ENative]::PostDropFiles($Hwnd, $Paths)
    if (-not $ok) {
        throw "跨进程投递 WM_DROPFILES 失败"
    }
}

function Send-MouseDragSelect {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Hwnd,
        [int]$StartX = 120,
        [int]$StartY = 120,
        [int]$EndX = 420,
        [int]$EndY = 180
    )
    [E2ENative]::DragMouseClient($Hwnd, $StartX, $StartY, $EndX, $EndY)
}

function Send-MouseWheelScroll {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Hwnd,
        [int]$Delta = -360
    )
    [E2ENative]::SendMouseWheel($Hwnd, $Delta)
}

function Find-ModalDialog {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [IntPtr]$MainHwnd = [IntPtr]::Zero,
        [int]$TimeoutMs = 3000
    )
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $ProcessId
    )
    do {
        try {
            # 1. 先检查同 PID 顶层窗口（排除主窗口与忽略辅助窗）
            $wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
            foreach ($w in $wins) {
                $cls = $w.Current.ClassName
                if ($cls -in $script:IgnoredWindowClasses) { continue }
                $wh = [IntPtr]$w.Current.NativeWindowHandle
                if ($MainHwnd -ne [IntPtr]::Zero -and $wh -eq $MainHwnd) { continue }
                if ($w.Current.BoundingRectangle.Width -lt 80) { continue }
                if ($cls -eq "#32770" -or $w.Current.ControlType -eq [System.Windows.Automation.ControlType]::Window) {
                    return $w
                }
            }
            # 2. 检查主窗口下的子模态窗口（部分 MessageBox / OpenFileDialog 挂在主窗口 Children 下）
            if ($MainHwnd -ne [IntPtr]::Zero) {
                $mainEl = [System.Windows.Automation.AutomationElement]::FromHandle($MainHwnd)
                $children = $mainEl.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
                foreach ($ch in $children) {
                    $ccls = $ch.Current.ClassName
                    if ($ccls -eq "#32770" -or $ch.Current.ControlType -eq [System.Windows.Automation.ControlType]::Window) {
                        return $ch
                    }
                }
            }
        } catch {}
        if ($TimeoutMs -le 0 -or $sw.ElapsedMilliseconds -ge $TimeoutMs) { break }
        Start-Sleep -Milliseconds 80
    } while ($true)
    return $null
}

function Get-ModalDialogText {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Modal
    )
    $parts = New-Object 'System.Collections.Generic.List[string]'
    if ($Modal.Current.Name) { [void]$parts.Add($Modal.Current.Name) }
    $all = $Modal.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($el in $all) {
        if ($el.Current.ControlType -eq [System.Windows.Automation.ControlType]::Text -and $el.Current.Name) {
            [void]$parts.Add($el.Current.Name)
        }
    }
    return ($parts -join "`n")
}

function Invoke-ModalBucket {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Modal,
        [Parameter(Mandatory = $true)][ValidateSet('Save', 'Discard', 'Cancel', 'Ok')][string]$Bucket
    )
    $all = $Modal.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $candidates = foreach ($el in $all) {
        $ct = $el.Current.ControlType
        $cls = $el.Current.ClassName
        if (($ct -eq [System.Windows.Automation.ControlType]::Button) -or ($cls -match 'Button|BUTTON')) {
            $el
        }
    }

    $target = $null
    foreach ($btn in $candidates) {
        $aid = $btn.Current.AutomationId
        $name = $btn.Current.Name
        switch ($Bucket) {
            'Save' {
                if ($aid -eq "6" -or ($name -notmatch '不保存' -and $name -match '保存|是\b|^Yes')) { $target = $btn; break }
            }
            'Discard' {
                if ($aid -eq "7" -or ($name -match '放弃|不保存|否\b|^No|Discard')) { $target = $btn; break }
            }
            'Cancel' {
                if ($aid -eq "2" -or ($name -match '取消|Cancel')) { $target = $btn; break }
            }
            'Ok' {
                if ($aid -eq "1" -or ($name -match '确定|OK|确认|关闭|Close')) { $target = $btn; break }
            }
        }
        if ($target) { break }
    }

    if (-not $target) {
        throw "模态框内未找到匹配 '$Bucket' 语义桶的按钮 (Title='$($Modal.Current.Name)')"
    }

    $modalHwnd = [IntPtr]$Modal.Current.NativeWindowHandle
    Invoke-Button -Element $target -MainHwnd $modalHwnd
}

function Invoke-AppCli {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$ExePath,
        [string[]]$Arguments = @(),
        [int]$TimeoutMs = 5000,
        [string]$WorkingDirectory = ""
    )
    $hJob = Initialize-E2EJobObject
    $resolvedExe = (Resolve-Path -LiteralPath $ExePath).Path
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $resolvedExe
    $psi.Arguments = Format-CliArgumentsString -Arguments $Arguments
    $psi.WorkingDirectory = if ($WorkingDirectory) { $WorkingDirectory } else { Split-Path -Parent $resolvedExe }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8
    $psi.CreateNoWindow = $true

    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi

    $outLines = [System.Collections.ArrayList]::Synchronized((New-Object System.Collections.ArrayList))
    $errLines = [System.Collections.ArrayList]::Synchronized((New-Object System.Collections.ArrayList))

    $outEvt = Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -Action {
        if ($null -ne $EventArgs.Data) { [void]$Event.MessageData.Add($EventArgs.Data) }
    } -MessageData $outLines
    $errEvt = Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -Action {
        if ($null -ne $EventArgs.Data) { [void]$Event.MessageData.Add($EventArgs.Data) }
    } -MessageData $errLines

    $hadModal = $false
    $modalText = ""
    $timedOut = $false
    $exitCode = -1

    try {
        [void]$proc.Start()
        [void]$script:TrackedPids.Add($proc.Id)
        try { [void][E2ENative]::AssignProcessToJobObject($hJob, $proc.Handle) } catch {}
        $proc.BeginOutputReadLine()
        $proc.BeginErrorReadLine()

        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        while (-not $proc.HasExited) {
            if ($sw.ElapsedMilliseconds -ge $TimeoutMs) {
                $timedOut = $true
                break
            }
            $modal = Find-ModalDialog -ProcessId $proc.Id -TimeoutMs 0
            if ($null -ne $modal) {
                $hadModal = $true
                $modalText = Get-ModalDialogText -Modal $modal
                try {
                    Invoke-ModalBucket -Modal $modal -Bucket Ok
                } catch {
                    $mh = [IntPtr]$modal.Current.NativeWindowHandle
                    if ($mh -ne [IntPtr]::Zero) {
                        [void][E2ENative]::PostMessage($mh, [E2ENative]::WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
                    }
                }
            }
            Start-Sleep -Milliseconds 60
        }

        if ($timedOut) {
            Stop-ProcessTreeById -ProcessId $proc.Id
        } else {
            # 等待异步输出事件冲刷完成
            $proc.WaitForExit()
            Start-Sleep -Milliseconds 50
            $exitCode = $proc.ExitCode
            [void]$script:TrackedPids.Remove($proc.Id)
        }
    } finally {
        if ($outEvt) { Unregister-Event -SourceIdentifier $outEvt.Name -ErrorAction SilentlyContinue; Remove-Job -Id $outEvt.Id -Force -ErrorAction SilentlyContinue }
        if ($errEvt) { Unregister-Event -SourceIdentifier $errEvt.Name -ErrorAction SilentlyContinue; Remove-Job -Id $errEvt.Id -Force -ErrorAction SilentlyContinue }
        $proc.Dispose()
    }

    return @{
        ExitCode           = $exitCode
        StdOut             = ($outLines -join "`n")
        StdErr             = ($errLines -join "`n")
        HadUnexpectedModal = $hadModal
        ModalText          = $modalText
        TimedOut           = $timedOut
    }
}

# ==============================================================================
# HKCU 三分区零污染注册表沙盒（严禁触碰 Explorer\FileExts\*\UserChoice*）
# ==============================================================================

function Invoke-RegExe {
    param([string[]]$Arguments)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "reg.exe"
    $psi.Arguments = Format-CliArgumentsString -Arguments $Arguments
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $stdout = $p.StandardOutput.ReadToEnd()
    $stderr = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    return @{
        ExitCode = $p.ExitCode
        StdOut   = $stdout
        StdErr   = $stderr
    }
}

function Export-RegKeyIfExists {
    param(
        [string]$RegPath,
        [string]$OutFile
    )
    $psPath = "Registry::$RegPath"
    if (-not (Test-Path -LiteralPath $psPath)) {
        return $false
    }
    $res = Invoke-RegExe -Arguments @('export', $RegPath, $OutFile, '/y')
    if ($res.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $OutFile)) {
        throw "导出注册表快照失败: $RegPath -> $OutFile (Exit=$($res.ExitCode), Err=$($res.StdErr))"
    }
    $len = (Get-Item -LiteralPath $OutFile).Length
    if ($len -le 0) {
        throw "导出的注册表快照为空文件: $OutFile"
    }
    return $true
}

function Restore-RegKeyPartition {
    param(
        [string]$RegPath,
        [bool]$Existed,
        [string]$SnapFile,
        [bool]$IsSharedKey
    )
    if ($Existed) {
        if (-not $SnapFile -or -not (Test-Path -LiteralPath $SnapFile) -or (Get-Item -LiteralPath $SnapFile).Length -le 0) {
            throw "回滚共享/已有键失败：缺少有效快照文件 $SnapFile ($RegPath)"
        }
        $null = Invoke-RegExe -Arguments @('delete', $RegPath, '/f')
        $resImp = Invoke-RegExe -Arguments @('import', $SnapFile)
        if ($resImp.ExitCode -ne 0) {
            throw "reg.exe import 回滚失败: $RegPath <- $SnapFile (Exit=$($resImp.ExitCode), Err=$($resImp.StdErr))"
        }
    } else {
        if (Test-Path -LiteralPath "Registry::$RegPath") {
            $resDel = Invoke-RegExe -Arguments @('delete', $RegPath, '/f')
            if ($resDel.ExitCode -ne 0 -and (Test-Path -LiteralPath "Registry::$RegPath")) {
                throw "删除测试期新增注册表键失败: $RegPath (Exit=$($resDel.ExitCode), Err=$($resDel.StdErr))"
            }
        }
    }
}

function Enter-HkcuRegistrySandbox {
    [CmdletBinding()]
    param()
    # 先兜底清理残留崩溃快照
    $null = Restore-DanglingRegistryIfAny

    $snapDir = Join-Path $env:TEMP ("mdreader_reg_snap_" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $snapDir | Out-Null

    # 分区 A：自建独占键
    $exclusiveKeys = @(
        "HKCU\Software\Classes\MDReader.Document",
        "HKCU\Software\Classes\MDReader.md",
        "HKCU\Software\Classes\Applications\mdreader.exe",
        "HKCU\Software\MDReader"
    )
    $partA = @()
    $idx = 0
    foreach ($k in $exclusiveKeys) {
        $idx++
        $file = Join-Path $snapDir ("partA_${idx}.reg")
        $exists = Export-RegKeyIfExists -RegPath $k -OutFile $file
        $partA += @{
            RegPath  = $k
            Existed  = $exists
            SnapFile = if ($exists) { $file } else { "" }
        }
    }

    # 分区 B：已有用户扩展名共享键
    $sharedExtKeys = @(
        "HKCU\Software\Classes\.md",
        "HKCU\Software\Classes\.markdown"
    )
    $partB = @()
    $idx = 0
    foreach ($k in $sharedExtKeys) {
        $idx++
        $file = Join-Path $snapDir ("partB_${idx}.reg")
        $exists = Export-RegKeyIfExists -RegPath $k -OutFile $file
        $partB += @{
            RegPath  = $k
            Existed  = $exists
            SnapFile = if ($exists) { $file } else { "" }
        }
    }

    # 分区 C：全局多应用共享容器键（值级快照，禁止整键删除）
    $regAppsAuditFile = Join-Path $snapDir "partC_RegisteredApplications_audit.reg"
    [void](Export-RegKeyIfExists -RegPath "HKCU\Software\RegisteredApplications" -OutFile $regAppsAuditFile)
    $watchedValueNames = @("MDReader", "MDReader.Document")
    $partCValues = @()
    $regAppsKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\RegisteredApplications", $false)
    try {
        foreach ($vn in $watchedValueNames) {
            if ($null -ne $regAppsKey -and ($regAppsKey.GetValueNames() -contains $vn)) {
                $val = $regAppsKey.GetValue($vn, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                $kind = $regAppsKey.GetValueKind($vn).ToString()
                $partCValues += @{ Name = $vn; Exists = $true; Kind = $kind; Value = [string]$val }
            } else {
                $partCValues += @{ Name = $vn; Exists = $false; Kind = ""; Value = "" }
            }
        }
    } finally {
        if ($null -ne $regAppsKey) { $regAppsKey.Dispose() }
    }

    $snapshot = @{
        CreatedUtc  = (Get-Date).ToUniversalTime().ToString("o")
        SnapDir     = $snapDir
        PartitionA  = $partA
        PartitionB  = $partB
        PartitionC  = $partCValues
        AuditFileC  = $regAppsAuditFile
    }

    $json = $snapshot | ConvertTo-Json -Depth 6
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllBytes($script:RegRecoveryManifestPath, $utf8NoBom.GetBytes($json))
    return $snapshot
}

function Restore-HkcuRegistrySandbox {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][hashtable]$Snapshot
    )
    $errors = New-Object 'System.Collections.Generic.List[string]'

    # 1. 恢复分区 C（值级恢复）
    try {
        if ($Snapshot.PartitionC) {
            $rwKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\RegisteredApplications", $true)
            if ($null -ne $rwKey) {
                try {
                    foreach ($item in $Snapshot.PartitionC) {
                        if ($item.Exists) {
                            $kindEnum = [Microsoft.Win32.RegistryValueKind]::Parse([Microsoft.Win32.RegistryValueKind], [string]$item.Kind)
                            $rwKey.SetValue([string]$item.Name, $item.Value, $kindEnum)
                        } else {
                            $rwKey.DeleteValue([string]$item.Name, $false)
                        }
                    }
                } finally {
                    $rwKey.Dispose()
                }
            }
        }
    } catch {
        [void]$errors.Add("分区 C 回滚异常: $($_.Exception.Message)")
    }

    # 2. 恢复分区 A（独占键）
    foreach ($entry in @($Snapshot.PartitionA)) {
        try {
            Restore-RegKeyPartition -RegPath $entry.RegPath -Existed ([bool]$entry.Existed) -SnapFile $entry.SnapFile -IsSharedKey $false
        } catch {
            [void]$errors.Add("分区 A 回滚异常 ($($entry.RegPath)): $($_.Exception.Message)")
        }
    }

    # 3. 恢复分区 B（已有扩展名共享键）
    foreach ($entry in @($Snapshot.PartitionB)) {
        try {
            Restore-RegKeyPartition -RegPath $entry.RegPath -Existed ([bool]$entry.Existed) -SnapFile $entry.SnapFile -IsSharedKey $true
        } catch {
            [void]$errors.Add("分区 B 回滚异常 ($($entry.RegPath)): $($_.Exception.Message)")
        }
    }

    if ($errors.Count -gt 0) {
        throw ("HKCU 注册表沙盒回滚发生错误:`n" + ($errors -join "`n"))
    }

    if (Test-Path -LiteralPath $script:RegRecoveryManifestPath) {
        Remove-Item -LiteralPath $script:RegRecoveryManifestPath -Force -ErrorAction SilentlyContinue
    }
    if ($Snapshot.SnapDir -and (Test-Path -LiteralPath $Snapshot.SnapDir)) {
        Remove-ReadOnlyRecursive -TargetDir $Snapshot.SnapDir
    }
}

function Convert-PsObjectToHashtable {
    param($Obj)
    if ($null -eq $Obj) { return $null }
    if ($Obj -is [System.Collections.IDictionary]) {
        $ht = @{}
        foreach ($k in $Obj.Keys) { $ht[$k] = Convert-PsObjectToHashtable $Obj[$k] }
        return $ht
    }
    if ($Obj -is [System.Management.Automation.PSCustomObject]) {
        $ht = @{}
        foreach ($p in $Obj.PSObject.Properties) {
            $ht[$p.Name] = Convert-PsObjectToHashtable $p.Value
        }
        return $ht
    }
    if ($Obj -is [System.Collections.IEnumerable] -and -not ($Obj -is [string])) {
        $arr = @()
        foreach ($item in $Obj) { $arr += ,(Convert-PsObjectToHashtable $item) }
        return $arr
    }
    return $Obj
}

function Restore-DanglingRegistryIfAny {
    [CmdletBinding()]
    param()
    if (-not (Test-Path -LiteralPath $script:RegRecoveryManifestPath)) {
        return $false
    }
    $raw = [System.IO.File]::ReadAllText($script:RegRecoveryManifestPath, [System.Text.Encoding]::UTF8)
    $parsed = $raw | ConvertFrom-Json
    $ht = Convert-PsObjectToHashtable $parsed
    Restore-HkcuRegistrySandbox -Snapshot $ht
    return $true
}

# ==============================================================================
# 字节级 UTF-8 / BOM / EOL 工具
# ==============================================================================

function Write-BytesUtf8 {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [bool]$Bom = $false,
        [ValidateSet('LF', 'CRLF')][string]$Eol = 'LF'
    )
    $normalized = $Text -replace "`r`n", "`n"
    $normalized = $normalized -replace "`r", "`n"
    if ($Eol -eq 'CRLF') {
        $normalized = $normalized -replace "`n", "`r`n"
    }
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    $bodyBytes = $utf8NoBom.GetBytes($normalized)
    if ($Bom) {
        $bomBytes = [byte[]](0xEF, 0xBB, 0xBF)
        $all = New-Object byte[] ($bomBytes.Length + $bodyBytes.Length)
        [Array]::Copy($bomBytes, 0, $all, 0, $bomBytes.Length)
        [Array]::Copy($bodyBytes, 0, $all, $bomBytes.Length, $bodyBytes.Length)
    } else {
        $all = $bodyBytes
    }
    $parent = Split-Path -Parent $Path
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    [System.IO.File]::WriteAllBytes($Path, $all)
}

function Read-FileBytesMeta {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Path
    )
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $hasBom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
    $startIdx = if ($hasBom) { 3 } else { 0 }

    $hasCrlf = $false
    $hasBareLf = $false
    for ($i = $startIdx; $i -lt $bytes.Length; $i++) {
        if ($bytes[$i] -eq 0x0A) {
            if ($i -gt $startIdx -and $bytes[$i - 1] -eq 0x0D) {
                $hasCrlf = $true
            } else {
                $hasBareLf = $true
            }
        }
    }

    $eolKind = if ($hasCrlf -and -not $hasBareLf) {
        'CRLF'
    } elseif ($hasBareLf -and -not $hasCrlf) {
        'LF'
    } elseif ($hasCrlf -and $hasBareLf) {
        'Mixed'
    } else {
        'None'
    }

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    $text = $utf8NoBom.GetString($bytes, $startIdx, $bytes.Length - $startIdx)

    return @{
        Path       = $Path
        Length     = $bytes.Length
        HasUtf8Bom = $hasBom
        HasCrLf    = $hasCrlf
        HasLf      = ($hasCrlf -or $hasBareLf)
        LineEnding = $eolKind
        Text       = $text
        RawBytes   = $bytes
    }
}

# ==============================================================================
# 用例注册、等待与断言
# ==============================================================================

function New-TestCaseDef {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][int]$Tier,
        [Parameter(Mandatory = $true)][string]$Feature,
        [Parameter(Mandatory = $true)][string]$Title,
        [string]$Group = "G4",
        [Parameter(Mandatory = $true)][scriptblock]$Run
    )
    return [PSCustomObject]@{
        Id      = $Id
        Tier    = $Tier
        Feature = $Feature
        Title   = $Title
        Group   = $Group
        Run     = $Run
    }
}

function Wait-Condition {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][scriptblock]$Condition,
        [int]$TimeoutMs = 8000,
        [int]$PollMs = 80,
        [string]$ErrorMessage = "等待条件超时"
    )
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $lastErr = $null
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try {
            $res = & $Condition
            if ($res) { return $res }
        } catch {
            $lastErr = $_.Exception.Message
        }
        Start-Sleep -Milliseconds $PollMs
    }
    if ($lastErr) {
        throw "${ErrorMessage} (${TimeoutMs}ms; LastError=$lastErr)"
    }
    throw "${ErrorMessage} (${TimeoutMs}ms)"
}

function Assert-True {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]$Condition,
        [string]$Message = "期望条件为 True，实际为 False"
    )
    if (-not [bool]$Condition) {
        throw "断言失败 [Assert-True]: $Message"
    }
}

function Assert-Equal {
    [CmdletBinding()]
    param(
        [AllowNull()]$Actual,
        [AllowNull()]$Expected,
        [string]$Message = ""
    )
    $eq = $false
    if ($null -eq $Expected -and $null -eq $Actual) {
        $eq = $true
    } elseif ($null -ne $Expected -and $null -ne $Actual) {
        $eq = ($Actual -ceq $Expected)
        if (-not $eq -and ($Expected -isnot [string]) -and ($Actual -isnot [string])) {
            $eq = ($Actual -eq $Expected)
        }
    }
    if (-not $eq) {
        $prefix = if ($Message) { "$Message — " } else { "" }
        throw "断言失败 [Assert-Equal]: ${prefix}期望=[$Expected], 实际=[$Actual]"
    }
}

function Assert-Match {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [Parameter(Mandatory = $true)][string]$Pattern,
        [string]$Message = ""
    )
    if ($Text -notmatch $Pattern) {
        $prefix = if ($Message) { "$Message — " } else { "" }
        throw "断言失败 [Assert-Match]: ${prefix}文本未匹配正则 [$Pattern], 实际文本=[$Text]"
    }
}

function Assert-Throws {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][scriptblock]$ScriptBlock,
        [string]$Pattern = "",
        [string]$Message = ""
    )
    $threw = $false
    $errMsg = ""
    try {
        & $ScriptBlock
    } catch {
        $threw = $true
        $errMsg = $_.Exception.Message
    }
    if (-not $threw) {
        $prefix = if ($Message) { "$Message — " } else { "" }
        throw "断言失败 [Assert-Throws]: ${prefix}期望抛出异常，但正常返回"
    }
    if ($Pattern -and ($errMsg -notmatch $Pattern)) {
        throw "断言失败 [Assert-Throws]: 异常消息 [$errMsg] 未匹配 [$Pattern]"
    }
}

Export-ModuleMember -Function @(
    'Initialize-E2EJobObject',
    'Close-E2EJobObject',
    'New-E2ECaseContext',
    'Remove-E2ECaseContext',
    'Start-AppInstance',
    'Stop-AppInstance',
    'Invoke-AppCleanupAll',
    'Get-ProcessTreeMetrics',
    'Find-MainWindow',
    'Find-Control',
    'Get-ToolbarActionButtons',
    'Invoke-Button',
    'Send-Hotkey',
    'Get-EditorText',
    'Set-EditorText',
    'Set-ClipboardTextWin32',
    'Get-ClipboardTextWin32',
    'Get-WindowLuminance',
    'Capture-WindowPng',
    'Compare-WindowCaptureDiff',
    'Send-DropFiles',
    'Send-MouseDragSelect',
    'Send-MouseWheelScroll',
    'Find-ModalDialog',
    'Get-ModalDialogText',
    'Invoke-ModalBucket',
    'Invoke-AppCli',
    'Enter-HkcuRegistrySandbox',
    'Restore-HkcuRegistrySandbox',
    'Restore-DanglingRegistryIfAny',
    'Write-BytesUtf8',
    'Read-FileBytesMeta',
    'New-TestCaseDef',
    'Wait-Condition',
    'Assert-True',
    'Assert-Equal',
    'Assert-Match',
    'Assert-Throws'
)
