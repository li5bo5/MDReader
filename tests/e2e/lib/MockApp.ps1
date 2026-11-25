#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader E2E 框架自检专用轻量替身 (MockApp.ps1)
.DESCRIPTION
    仅在 `-SelfTest` 模式下使用，通过 Add-Type -OutputAssembly 编译为独立 WindowsApplication (.exe)，
    用于验证 E2EHarness.psm1 与 run-e2e.ps1 自身的 UIA 驱动、消息投递、HKCU 三分区沙盒、
    字节级 BOM/EOL 原子写入、PrintWindow 亮暗采样与进程防游离清理链路。
    正式针对被测程序运行时（-ExePath）严禁使用本替身。
#>

param(
    [switch]$CompileOnly,
    [string]$OutExePath = ""
)

Set-StrictMode -Version Latest

$script:MockAppCSharpSource = @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using Microsoft.Win32;

namespace MDReaderMock {
    internal static class NativeWin32 {
        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        public static extern void DragAcceptFiles(IntPtr hWnd, bool fAccept);

        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        public static extern uint DragQueryFile(IntPtr hDrop, uint iFile, StringBuilder lpszFile, uint cch);

        [DllImport("shell32.dll")]
        public static extern void DragFinish(IntPtr hDrop);

        [DllImport("user32.dll")]
        public static extern bool SetProcessDPIAware();
    }

    internal enum AppMode {
        Reading,
        Editing
    }

    internal enum LineEndingKind {
        Lf,
        Crlf
    }

    internal sealed class MockConfig {
        public string Theme = "light";
        public double FontSize = 16;
        public int MaxWidth = 760;
        public int PagePadding = 40;
        public double EditorFontSize = 15;
        public int EditorTabSize = 4;
        public int WindowWidth = 960;
        public int WindowHeight = 760;
        public Color BgColor = ColorTranslator.FromHtml("#FAF8F5");
        public Color TextColor = ColorTranslator.FromHtml("#1F2328");
        public Color CodeBgColor = ColorTranslator.FromHtml("#F1EFEA");
        public Color QuoteBgColor = ColorTranslator.FromHtml("#F4F1EA");
        public Color LinkColor = ColorTranslator.FromHtml("#0969DA");
        public Color BorderColor = ColorTranslator.FromHtml("#D0D7DE");

        public static MockConfig LoadWithFallback(string cliConfigPath, string exeDir) {
            MockConfig cfg = new MockConfig();
            string resolved = ResolveConfigPath(cliConfigPath, exeDir);
            if (string.IsNullOrEmpty(resolved) || !File.Exists(resolved)) {
                cfg.ApplyThemeDefaults();
                return cfg;
            }
            try {
                string raw = File.ReadAllText(resolved, Encoding.UTF8);
                if (string.IsNullOrWhiteSpace(raw)) {
                    cfg.ApplyThemeDefaults();
                    return cfg;
                }
                JavaScriptSerializer jss = new JavaScriptSerializer();
                object parsed = jss.DeserializeObject(raw);
                Dictionary<string, object> dict = parsed as Dictionary<string, object>;
                if (dict == null) {
                    cfg.ApplyThemeDefaults();
                    return cfg;
                }

                string t = GetString(dict, "theme");
                if (!string.IsNullOrEmpty(t)) {
                    string tl = t.Trim().ToLowerInvariant();
                    if (tl == "light" || tl == "dark" || tl == "system") {
                        cfg.Theme = tl;
                    }
                }
                cfg.ApplyThemeDefaults();

                double fs;
                if (TryGetDouble(dict, out fs, "fontSize", "font_size") && fs >= 12 && fs <= 32) cfg.FontSize = fs;

                int mw;
                if (TryGetInt(dict, out mw, "maxWidth", "max_width") && mw >= 480 && mw <= 1400) cfg.MaxWidth = mw;

                int pp;
                if (TryGetInt(dict, out pp, "pagePadding", "page_padding") && pp >= 12 && pp <= 120) cfg.PagePadding = pp;

                double efs;
                if (TryGetDouble(dict, out efs, "editorFontSize", "editor_font_size") && efs >= 12 && efs <= 28) cfg.EditorFontSize = efs;

                int ets;
                if (TryGetInt(dict, out ets, "editorTabSize", "editor_tab_size") && ets >= 2 && ets <= 8) cfg.EditorTabSize = ets;

                int ww;
                if (TryGetInt(dict, out ww, "windowWidth", "window_width") && ww >= 400 && ww <= 3840) cfg.WindowWidth = ww;

                int wh;
                if (TryGetInt(dict, out wh, "windowHeight", "window_height") && wh >= 300 && wh <= 2160) cfg.WindowHeight = wh;

                object colorsObj;
                if (dict.TryGetValue("colors", out colorsObj)) {
                    Dictionary<string, object> colors = colorsObj as Dictionary<string, object>;
                    if (colors != null) {
                        Color c;
                        if (TryParseHex(GetString(colors, "background"), out c)) cfg.BgColor = c;
                        if (TryParseHex(GetString(colors, "text"), out c)) cfg.TextColor = c;
                        if (TryParseHex(GetString(colors, "codeBackground"), out c)) cfg.CodeBgColor = c;
                        if (TryParseHex(GetString(colors, "quoteBackground"), out c)) cfg.QuoteBgColor = c;
                        if (TryParseHex(GetString(colors, "link"), out c)) cfg.LinkColor = c;
                        if (TryParseHex(GetString(colors, "border"), out c)) cfg.BorderColor = c;
                    }
                }
            } catch {
                cfg.ApplyThemeDefaults();
            }
            return cfg;
        }

        private void ApplyThemeDefaults() {
            string effective = Theme;
            if (effective == "system") {
                effective = IsSystemLightTheme() ? "light" : "dark";
            }
            if (effective == "dark") {
                BgColor = ColorTranslator.FromHtml("#1E1E1E");
                TextColor = ColorTranslator.FromHtml("#E4E4E7");
                CodeBgColor = ColorTranslator.FromHtml("#2A2A2E");
                QuoteBgColor = ColorTranslator.FromHtml("#252529");
                LinkColor = ColorTranslator.FromHtml("#58A6FF");
                BorderColor = ColorTranslator.FromHtml("#3F3F46");
            } else {
                BgColor = ColorTranslator.FromHtml("#FAF8F5");
                TextColor = ColorTranslator.FromHtml("#1F2328");
                CodeBgColor = ColorTranslator.FromHtml("#F1EFEA");
                QuoteBgColor = ColorTranslator.FromHtml("#F4F1EA");
                LinkColor = ColorTranslator.FromHtml("#0969DA");
                BorderColor = ColorTranslator.FromHtml("#D0D7DE");
            }
        }

        private static bool IsSystemLightTheme() {
            try {
                using (RegistryKey k = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", false)) {
                    if (k != null) {
                        object v = k.GetValue("AppsUseLightTheme");
                        if (v is int) return ((int)v) != 0;
                    }
                }
            } catch { }
            return true;
        }

        public static string ResolveConfigPath(string cliConfigPath, string exeDir) {
            if (!string.IsNullOrEmpty(cliConfigPath)) return cliConfigPath;
            string portable = Path.Combine(exeDir, "config.json");
            if (File.Exists(portable)) return portable;
            string appData = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "MDReader", "config.json");
            return appData;
        }

        private static string GetString(Dictionary<string, object> dict, params string[] keys) {
            foreach (string k in keys) {
                object val;
                if (dict.TryGetValue(k, out val) && val is string) {
                    return (string)val;
                }
            }
            return null;
        }

        private static bool TryGetDouble(Dictionary<string, object> dict, out double value, params string[] keys) {
            value = 0;
            foreach (string k in keys) {
                object val;
                if (dict.TryGetValue(k, out val) && val != null && !(val is bool) && !(val is string)) {
                    try {
                        value = Convert.ToDouble(val);
                        return true;
                    } catch { }
                }
            }
            return false;
        }

        private static bool TryGetInt(Dictionary<string, object> dict, out int value, params string[] keys) {
            value = 0;
            foreach (string k in keys) {
                object val;
                if (dict.TryGetValue(k, out val) && val != null && !(val is bool) && !(val is string)) {
                    try {
                        double d = Convert.ToDouble(val);
                        if (Math.Abs(d - Math.Round(d)) < 1e-6) {
                            value = (int)Math.Round(d);
                            return true;
                        }
                    } catch { }
                }
            }
            return false;
        }

        private static bool TryParseHex(string hex, out Color color) {
            color = Color.Empty;
            if (string.IsNullOrWhiteSpace(hex)) return false;
            string s = hex.Trim();
            if (!Regex.IsMatch(s, "^#([0-9A-Fa-f]{6}|[0-9A-Fa-f]{8})$")) return false;
            try {
                if (s.Length == 7) {
                    color = ColorTranslator.FromHtml(s);
                    return true;
                }
                int r = Convert.ToInt32(s.Substring(1, 2), 16);
                int g = Convert.ToInt32(s.Substring(3, 2), 16);
                int b = Convert.ToInt32(s.Substring(5, 2), 16);
                int a = Convert.ToInt32(s.Substring(7, 2), 16);
                color = Color.FromArgb(a, r, g, b);
                return true;
            } catch {
                return false;
            }
        }
    }

    internal sealed class ReaderCanvasPanel : Panel {
        private const int WM_PRINTCLIENT = 0x0318;
        public string DocumentText = "";
        public string ErrorBanner = "";
        public MockConfig Config = new MockConfig();
        public int ScrollOffset = 0;
        public string FindQuery = "";
        public int ActiveMatchIndex = -1;
        public List<int> MatchPositions = new List<int>();
        public bool HasSelection = false;
        public string SelectedText = "";
        private bool _dragging = false;
        private Point _dragStart;

        public ReaderCanvasPanel() {
            DoubleBuffered = true;
            SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.UserPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
        }

        public void UpdateMatches(string query) {
            FindQuery = query ?? "";
            MatchPositions.Clear();
            ActiveMatchIndex = -1;
            if (!string.IsNullOrEmpty(FindQuery) && !string.IsNullOrEmpty(DocumentText)) {
                int idx = 0;
                while ((idx = DocumentText.IndexOf(FindQuery, idx, StringComparison.OrdinalIgnoreCase)) >= 0) {
                    MatchPositions.Add(idx);
                    idx += FindQuery.Length;
                }
                if (MatchPositions.Count > 0) ActiveMatchIndex = 0;
            }
            Invalidate();
        }

        public void StepMatch(bool forward) {
            if (MatchPositions.Count == 0) return;
            if (forward) {
                ActiveMatchIndex = (ActiveMatchIndex + 1) % MatchPositions.Count;
            } else {
                ActiveMatchIndex = (ActiveMatchIndex - 1 + MatchPositions.Count) % MatchPositions.Count;
            }
            Invalidate();
        }

        protected override void OnMouseDown(MouseEventArgs e) {
            base.OnMouseDown(e);
            if (e.Button == MouseButtons.Left) {
                _dragging = true;
                _dragStart = e.Location;
                HasSelection = false;
                SelectedText = "";
                Invalidate();
            }
        }

        protected override void OnMouseUp(MouseEventArgs e) {
            base.OnMouseUp(e);
            if (_dragging) {
                _dragging = false;
                int dist = Math.Abs(e.X - _dragStart.X) + Math.Abs(e.Y - _dragStart.Y);
                if (dist > 10 && !string.IsNullOrEmpty(DocumentText)) {
                    HasSelection = true;
                    string plain = DocumentText.Trim();
                    int len = Math.Min(plain.Length, 120);
                    SelectedText = plain.Substring(0, len);
                }
                Invalidate();
            }
        }

        protected override void OnMouseWheel(MouseEventArgs e) {
            base.OnMouseWheel(e);
            if (e.Delta < 0) ScrollOffset += 48;
            else ScrollOffset = Math.Max(0, ScrollOffset - 48);
            Invalidate();
        }

        protected override void WndProc(ref Message m) {
            base.WndProc(ref m);
            if (m.Msg == WM_PRINTCLIENT && m.WParam != IntPtr.Zero) {
                using (Graphics g = Graphics.FromHdc(m.WParam)) {
                    PaintContent(g);
                }
            }
        }

        protected override void OnPaintBackground(PaintEventArgs e) {
            using (SolidBrush br = new SolidBrush(Config.BgColor)) {
                e.Graphics.FillRectangle(br, ClientRectangle);
            }
        }

        protected override void OnPaint(PaintEventArgs e) {
            PaintContent(e.Graphics);
        }

        private void PaintContent(Graphics g) {
            using (SolidBrush bg = new SolidBrush(Config.BgColor)) {
                g.FillRectangle(bg, ClientRectangle);
            }
            int contentW = Math.Min(Config.MaxWidth, Math.Max(200, ClientSize.Width - Config.PagePadding * 2));
            int left = Math.Max(Config.PagePadding, (ClientSize.Width - contentW) / 2);
            int top = Math.Max(12, Config.PagePadding / 2) - ScrollOffset;

            if (!string.IsNullOrEmpty(ErrorBanner)) {
                using (Font ef = new Font("Microsoft YaHei UI", (float)Config.FontSize, FontStyle.Bold))
                using (SolidBrush eb = new SolidBrush(Color.Firebrick)) {
                    g.DrawString(ErrorBanner, ef, eb, new RectangleF(left, Math.Max(20, top), contentW, ClientSize.Height));
                }
                return;
            }

            string displayText = string.IsNullOrEmpty(DocumentText)
                ? "MDReader 极简阅读模式\n使用“打开”按钮或拖入 .md 文件开始阅读。"
                : DocumentText;

            // 去除潜在危险标签避免误导呈现
            displayText = Regex.Replace(displayText, "<script[^>]*>.*?</script>", "[已过滤脚本]", RegexOptions.IgnoreCase | RegexOptions.Singleline);

            if (HasSelection) {
                using (SolidBrush selBr = new SolidBrush(Color.FromArgb(60, Config.LinkColor))) {
                    g.FillRectangle(selBr, left, Math.Max(16, top), Math.Min(contentW, 380), 42);
                }
            }

            if (MatchPositions.Count > 0 && ActiveMatchIndex >= 0) {
                using (SolidBrush hiBr = new SolidBrush(Color.FromArgb(90, 255, 215, 0))) {
                    g.FillRectangle(hiBr, left, Math.Max(20, top + 8), Math.Min(contentW, 240), 26);
                }
            }

            using (Font bodyFont = new Font("Microsoft YaHei UI", (float)Config.FontSize, FontStyle.Regular))
            using (SolidBrush textBrush = new SolidBrush(Config.TextColor)) {
                RectangleF rect = new RectangleF(left, top, contentW, Math.Max(400, ClientSize.Height + ScrollOffset));
                g.DrawString(displayText, bodyFont, textBrush, rect);
            }
        }
    }

    internal sealed class MainForm : Form {
        private const int WM_DROPFILES = 0x0233;
        private readonly string _cliConfigPath;
        private readonly string _exeDir;

        private MockConfig _config;
        private AppMode _mode = AppMode.Reading;
        private string _currentFilePath = null;
        private string _savedText = "";
        private bool _hasUtf8Bom = false;
        private LineEndingKind _lineEnding = LineEndingKind.Lf;
        private bool _isDirty = false;
        private bool _suppressDirty = false;

        private Panel _toolbar;
        private Button _btnToggle;
        private Button _btnCancel;
        private Button _btnOpen;

        private Panel _findBar;
        private TextBox _txtFind;
        private Label _lblFindStatus;

        private ReaderCanvasPanel _readerPanel;
        private RichTextBox _editor;

        public MainForm(string initialFilePath, string cliConfigPath) {
            _cliConfigPath = cliConfigPath;
            _exeDir = Path.GetDirectoryName(Application.ExecutablePath) ?? Environment.CurrentDirectory;
            _config = MockConfig.LoadWithFallback(_cliConfigPath, _exeDir);

            KeyPreview = true;
            AllowDrop = true;
            Width = _config.WindowWidth;
            Height = _config.WindowHeight;
            StartPosition = FormStartPosition.CenterScreen;

            BuildUi();
            ApplyConfigToUi();

            if (!string.IsNullOrEmpty(initialFilePath)) {
                LoadFileIntoReader(initialFilePath);
            } else {
                _currentFilePath = null;
                _savedText = "";
                _readerPanel.DocumentText = "";
                _readerPanel.ErrorBanner = "";
                UpdateWindowTitle();
            }
        }

        protected override void OnHandleCreated(EventArgs e) {
            base.OnHandleCreated(e);
            NativeWin32.DragAcceptFiles(this.Handle, true);
        }

        private void BuildUi() {
            _toolbar = new Panel { Dock = DockStyle.Top, Height = 38, Padding = new Padding(8, 5, 8, 5) };

            _btnToggle = new Button {
                Name = "IDC_MODE_TOGGLE",
                Text = "编辑",
                Width = 104,
                Height = 28,
                Left = 8,
                Top = 5,
                FlatStyle = FlatStyle.System
            };
            _btnToggle.Click += delegate { HandleToggleModeClick(); };

            _btnCancel = new Button {
                Name = "IDC_EDIT_CANCEL",
                Text = "取消",
                Width = 72,
                Height = 28,
                Left = 118,
                Top = 5,
                Visible = false,
                FlatStyle = FlatStyle.System
            };
            _btnCancel.Click += delegate { HandleCancelEdit(); };

            _btnOpen = new Button {
                Name = "IDC_FILE_OPEN",
                Text = "打开",
                Width = 72,
                Height = 28,
                Left = 196,
                Top = 5,
                FlatStyle = FlatStyle.System
            };
            _btnOpen.Click += delegate { HandleOpenFileDialog(); };

            _toolbar.Controls.Add(_btnToggle);
            _toolbar.Controls.Add(_btnCancel);
            _toolbar.Controls.Add(_btnOpen);

            _findBar = new Panel { Dock = DockStyle.Top, Height = 34, Visible = false, Padding = new Padding(8, 4, 8, 4) };
            _txtFind = new TextBox { Name = "IDC_FIND_INPUT", Left = 8, Top = 5, Width = 260, Height = 24 };
            _txtFind.TextChanged += delegate {
                _readerPanel.UpdateMatches(_txtFind.Text);
                UpdateFindLabel();
            };
            _lblFindStatus = new Label { Name = "IDC_FIND_STATUS", Left = 278, Top = 8, Width = 160, Height = 20, Text = "" };
            _findBar.Controls.Add(_txtFind);
            _findBar.Controls.Add(_lblFindStatus);

            _readerPanel = new ReaderCanvasPanel { Name = "IDC_READER", Dock = DockStyle.Fill, Visible = true };

            _editor = new RichTextBox {
                Name = "IDC_EDITOR",
                Dock = DockStyle.Fill,
                Multiline = true,
                AcceptsTab = true,
                WordWrap = true,
                BorderStyle = BorderStyle.None,
                Visible = false
            };
            _editor.TextChanged += delegate {
                if (_suppressDirty) return;
                if (_mode == AppMode.Editing) {
                    _isDirty = true;
                    UpdateWindowTitle();
                }
            };

            Controls.Add(_readerPanel);
            Controls.Add(_editor);
            Controls.Add(_findBar);
            Controls.Add(_toolbar);
        }

        private void ApplyConfigToUi() {
            BackColor = _config.BgColor;
            _toolbar.BackColor = _config.CodeBgColor;
            _findBar.BackColor = _config.CodeBgColor;
            _lblFindStatus.ForeColor = _config.TextColor;

            _readerPanel.Config = _config;
            _readerPanel.BackColor = _config.BgColor;
            _readerPanel.Invalidate();

            _editor.BackColor = _config.BgColor;
            _editor.ForeColor = _config.TextColor;
            _editor.Font = new Font("Consolas", (float)_config.EditorFontSize, FontStyle.Regular);
        }

        private void UpdateFindLabel() {
            if (string.IsNullOrEmpty(_txtFind.Text)) {
                _lblFindStatus.Text = "";
                return;
            }
            if (_readerPanel.MatchPositions.Count == 0) {
                _lblFindStatus.Text = "0 命中";
            } else {
                _lblFindStatus.Text = string.Format("{0}/{1}", _readerPanel.ActiveMatchIndex + 1, _readerPanel.MatchPositions.Count);
            }
        }

        private void UpdateWindowTitle() {
            string filePart = string.IsNullOrEmpty(_currentFilePath) ? "未命名" : Path.GetFileName(_currentFilePath);
            string dirtyPart = (_mode == AppMode.Editing && _isDirty) ? "*" : "";
            string modePart = (_mode == AppMode.Reading) ? "阅读模式" : "修改模式";
            Text = string.Format("{0}{1} - {2} - MDReader", filePart, dirtyPart, modePart);
        }

        private void LoadFileIntoReader(string path) {
            try {
                if (!File.Exists(path)) {
                    _currentFilePath = path;
                    _savedText = "";
                    _isDirty = false;
                    _readerPanel.DocumentText = "";
                    _readerPanel.ErrorBanner = "文件不存在或无法访问: " + path;
                    SwitchToReadingModeImmediate();
                    return;
                }
                byte[] raw = File.ReadAllBytes(path);
                _hasUtf8Bom = (raw.Length >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF);
                int start = _hasUtf8Bom ? 3 : 0;

                bool hasCrLf = false;
                for (int i = start; i < raw.Length - 1; i++) {
                    if (raw[i] == 0x0D && raw[i + 1] == 0x0A) { hasCrLf = true; break; }
                }
                _lineEnding = hasCrLf ? LineEndingKind.Crlf : LineEndingKind.Lf;

                UTF8Encoding utf8 = new UTF8Encoding(false, false);
                string content = utf8.GetString(raw, start, raw.Length - start);
                _currentFilePath = Path.GetFullPath(path);
                _savedText = content;
                _isDirty = false;
                _readerPanel.ErrorBanner = "";
                _readerPanel.DocumentText = content;
                _readerPanel.ScrollOffset = 0;
                SwitchToReadingModeImmediate();
            } catch (Exception ex) {
                _readerPanel.ErrorBanner = "读取文件失败: " + ex.Message;
                _readerPanel.Invalidate();
            }
        }

        private void SwitchToReadingModeImmediate() {
            _mode = AppMode.Reading;
            _isDirty = false;
            _editor.Visible = false;
            _readerPanel.Visible = true;
            _btnToggle.Text = "编辑";
            _btnCancel.Visible = false;
            _readerPanel.Invalidate();
            UpdateWindowTitle();
        }

        private void EnterEditingMode() {
            if (string.IsNullOrEmpty(_currentFilePath) || !string.IsNullOrEmpty(_readerPanel.ErrorBanner)) {
                return;
            }
            _mode = AppMode.Editing;
            _findBar.Visible = false;
            _readerPanel.Visible = false;
            _editor.Visible = true;
            _btnToggle.Text = "保存并阅读";
            _btnCancel.Visible = true;

            _suppressDirty = true;
            try {
                _editor.Text = _savedText;
                _editor.ClearUndo();
            } finally {
                _suppressDirty = false;
            }
            _isDirty = false;
            UpdateWindowTitle();
            _editor.Focus();
        }

        private bool SaveCurrentDocumentAndSwitchToRead() {
            if (_mode != AppMode.Editing || string.IsNullOrEmpty(_currentFilePath)) return true;

            string editorText = _editor.Text.Replace("\r\n", "\n").Replace("\r", "\n");
            string normalized = (_lineEnding == LineEndingKind.Crlf)
                ? editorText.Replace("\n", "\r\n")
                : editorText;

            UTF8Encoding utf8NoBom = new UTF8Encoding(false);
            byte[] bodyBytes = utf8NoBom.GetBytes(normalized);
            byte[] outBytes;
            if (_hasUtf8Bom) {
                outBytes = new byte[3 + bodyBytes.Length];
                outBytes[0] = 0xEF; outBytes[1] = 0xBB; outBytes[2] = 0xBF;
                Array.Copy(bodyBytes, 0, outBytes, 3, bodyBytes.Length);
            } else {
                outBytes = bodyBytes;
            }

            string dir = Path.GetDirectoryName(_currentFilePath);
            string tmpPath = Path.Combine(dir, Path.GetFileName(_currentFilePath) + ".mdreader.tmp." + Guid.NewGuid().ToString("N").Substring(0, 8));
            try {
                if (!Directory.Exists(dir) || !File.Exists(_currentFilePath)) {
                    throw new IOException("目标文件已不存在或目录不可写: " + _currentFilePath);
                }
                FileAttributes attr = File.GetAttributes(_currentFilePath);
                if ((attr & FileAttributes.ReadOnly) != 0) {
                    throw new UnauthorizedAccessException("目标文件为只读属性，无法保存。");
                }
                using (FileStream fs = new FileStream(tmpPath, FileMode.CreateNew, FileAccess.Write, FileShare.None)) {
                    fs.Write(outBytes, 0, outBytes.Length);
                    fs.Flush(true);
                }
                File.Replace(tmpPath, _currentFilePath, null);
                _savedText = normalized;
                _isDirty = false;
                _readerPanel.DocumentText = _savedText;
                _readerPanel.ErrorBanner = "";
                SwitchToReadingModeImmediate();
                return true;
            } catch (Exception ex) {
                try { if (File.Exists(tmpPath)) File.Delete(tmpPath); } catch { }
                MessageBox.Show(this, "保存文件失败: " + ex.Message, "保存错误", MessageBoxButtons.OK, MessageBoxIcon.Error);
                return false;
            }
        }

        private bool ConfirmDiscardOrSaveIfDirty() {
            if (_mode != AppMode.Editing || !_isDirty) return true;
            DialogResult dr = MessageBox.Show(
                this,
                "当前修改尚未保存，是否保存更改？",
                "未保存确认",
                MessageBoxButtons.YesNoCancel,
                MessageBoxIcon.Warning
            );
            if (dr == DialogResult.Yes) {
                return SaveCurrentDocumentAndSwitchToRead();
            }
            if (dr == DialogResult.No) {
                _isDirty = false;
                return true;
            }
            return false;
        }

        private void HandleToggleModeClick() {
            if (_mode == AppMode.Reading) {
                EnterEditingMode();
            } else {
                SaveCurrentDocumentAndSwitchToRead();
            }
        }

        private void HandleCancelEdit() {
            if (_mode != AppMode.Editing) return;
            if (!_isDirty) {
                SwitchToReadingModeImmediate();
                return;
            }
            DialogResult dr = MessageBox.Show(
                this,
                "当前修改尚未保存，是否保存更改？",
                "未保存确认",
                MessageBoxButtons.YesNoCancel,
                MessageBoxIcon.Warning
            );
            if (dr == DialogResult.Yes) {
                SaveCurrentDocumentAndSwitchToRead();
            } else if (dr == DialogResult.No) {
                SwitchToReadingModeImmediate();
            }
        }

        private void HandleOpenFileDialog() {
            if (!ConfirmDiscardOrSaveIfDirty()) return;
            using (OpenFileDialog ofd = new OpenFileDialog()) {
                ofd.Filter = "Markdown Files (*.md;*.markdown)|*.md;*.markdown|All Files (*.*)|*.*";
                if (ofd.ShowDialog(this) == DialogResult.OK) {
                    LoadFileIntoReader(ofd.FileName);
                }
            }
        }

        protected override void OnFormClosing(FormClosingEventArgs e) {
            if (e.CloseReason == CloseReason.UserClosing && _mode == AppMode.Editing && _isDirty) {
                if (!ConfirmDiscardOrSaveIfDirty()) {
                    e.Cancel = true;
                    return;
                }
            }
            base.OnFormClosing(e);
        }

        protected override void WndProc(ref Message m) {
            if (m.Msg == WM_DROPFILES) {
                try {
                    uint count = NativeWin32.DragQueryFile(m.WParam, 0xFFFFFFFF, null, 0);
                    if (count > 0) {
                        StringBuilder sb = new StringBuilder(1024);
                        NativeWin32.DragQueryFile(m.WParam, 0, sb, (uint)sb.Capacity);
                        string dropped = sb.ToString();
                        NativeWin32.DragFinish(m.WParam);
                        if (!string.IsNullOrEmpty(dropped)) {
                            if (ConfirmDiscardOrSaveIfDirty()) {
                                LoadFileIntoReader(dropped);
                            }
                        }
                        return;
                    }
                    NativeWin32.DragFinish(m.WParam);
                } catch { }
            }
            base.WndProc(ref m);
        }

        protected override bool ProcessCmdKey(ref Message msg, Keys keyData) {
            if (keyData == (Keys.Control | Keys.E) || keyData == Keys.F2) {
                if (_mode == AppMode.Reading) {
                    EnterEditingMode();
                } else {
                    HandleCancelEdit();
                }
                return true;
            }
            if (keyData == (Keys.Control | Keys.S)) {
                if (_mode == AppMode.Editing) {
                    SaveCurrentDocumentAndSwitchToRead();
                }
                return true;
            }
            if (keyData == (Keys.Control | Keys.O)) {
                HandleOpenFileDialog();
                return true;
            }
            if (keyData == (Keys.Control | Keys.F)) {
                if (_mode == AppMode.Reading) {
                    _findBar.Visible = true;
                    _txtFind.Focus();
                    _txtFind.SelectAll();
                }
                return true;
            }
            if (keyData == (Keys.Control | Keys.Shift | Keys.R)) {
                _config = MockConfig.LoadWithFallback(_cliConfigPath, _exeDir);
                ApplyConfigToUi();
                return true;
            }
            if (keyData == (Keys.Control | Keys.C) && _mode == AppMode.Reading) {
                if (_readerPanel.HasSelection && !string.IsNullOrEmpty(_readerPanel.SelectedText)) {
                    Clipboard.SetText(_readerPanel.SelectedText);
                }
                return true;
            }
            if (keyData == Keys.Escape) {
                if (_findBar.Visible) {
                    _findBar.Visible = false;
                    _readerPanel.UpdateMatches("");
                    return true;
                }
                if (_mode == AppMode.Editing) {
                    HandleCancelEdit();
                    return true;
                }
            }
            if (_findBar.Visible && _txtFind.Focused) {
                if (keyData == Keys.Enter) {
                    _readerPanel.StepMatch(true);
                    UpdateFindLabel();
                    return true;
                }
                if (keyData == (Keys.Shift | Keys.Enter)) {
                    _readerPanel.StepMatch(false);
                    UpdateFindLabel();
                    return true;
                }
            }
            if (_mode == AppMode.Reading && !_findBar.Visible) {
                if (keyData == Keys.PageDown) {
                    _readerPanel.ScrollOffset += 200;
                    _readerPanel.Invalidate();
                    return true;
                }
                if (keyData == Keys.PageUp) {
                    _readerPanel.ScrollOffset = Math.Max(0, _readerPanel.ScrollOffset - 200);
                    _readerPanel.Invalidate();
                    return true;
                }
                if (keyData == Keys.Home) {
                    _readerPanel.ScrollOffset = 0;
                    _readerPanel.Invalidate();
                    return true;
                }
                if (keyData == Keys.End) {
                    _readerPanel.ScrollOffset += 1000;
                    _readerPanel.Invalidate();
                    return true;
                }
            }
            return base.ProcessCmdKey(ref msg, keyData);
        }
    }

    internal static class AssocManager {
        public static void Register(string exePath) {
            string absExe = Path.GetFullPath(exePath);
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\Classes\MDReader.Document")) {
                k.SetValue("", "Markdown Document", RegistryValueKind.String);
            }
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\Classes\MDReader.Document\DefaultIcon")) {
                k.SetValue("", string.Format("\"{0}\",0", absExe), RegistryValueKind.String);
            }
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\Classes\MDReader.Document\shell\open\command")) {
                k.SetValue("", string.Format("\"{0}\" \"%1\"", absExe), RegistryValueKind.String);
            }
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\Classes\.md\OpenWithProgids")) {
                k.SetValue("MDReader.Document", "", RegistryValueKind.String);
            }
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\Classes\.markdown\OpenWithProgids")) {
                k.SetValue("MDReader.Document", "", RegistryValueKind.String);
            }
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\MDReader\Capabilities")) {
                k.SetValue("ApplicationName", "MDReader", RegistryValueKind.String);
                k.SetValue("ApplicationDescription", "Pure low-memory Markdown reader", RegistryValueKind.String);
            }
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\MDReader\Capabilities\FileAssociations")) {
                k.SetValue(".md", "MDReader.Document", RegistryValueKind.String);
                k.SetValue(".markdown", "MDReader.Document", RegistryValueKind.String);
            }
            using (RegistryKey k = Registry.CurrentUser.CreateSubKey(@"Software\RegisteredApplications")) {
                k.SetValue("MDReader", @"Software\MDReader\Capabilities", RegistryValueKind.String);
            }
        }

        public static void Unregister() {
            Registry.CurrentUser.DeleteSubKeyTree(@"Software\Classes\MDReader.Document", false);
            Registry.CurrentUser.DeleteSubKeyTree(@"Software\MDReader", false);
            using (RegistryKey k = Registry.CurrentUser.OpenSubKey(@"Software\Classes\.md\OpenWithProgids", true)) {
                if (k != null) k.DeleteValue("MDReader.Document", false);
            }
            using (RegistryKey k = Registry.CurrentUser.OpenSubKey(@"Software\Classes\.markdown\OpenWithProgids", true)) {
                if (k != null) k.DeleteValue("MDReader.Document", false);
            }
            using (RegistryKey k = Registry.CurrentUser.OpenSubKey(@"Software\RegisteredApplications", true)) {
                if (k != null) k.DeleteValue("MDReader", false);
            }
        }
    }

    public static class Program {
        [STAThread]
        public static int Main(string[] args) {
            NativeWin32.SetProcessDPIAware();
            bool doHelp = false;
            bool doRegister = false;
            bool doUnregister = false;
            string configPath = null;
            string filePath = null;

            for (int i = 0; i < args.Length; i++) {
                string a = args[i];
                if (a == "--help" || a == "-h") {
                    doHelp = true;
                } else if (a == "--register") {
                    doRegister = true;
                } else if (a == "--unregister") {
                    doUnregister = true;
                } else if (a == "--config") {
                    if (i + 1 >= args.Length) {
                        Console.Error.WriteLine("参数错误: --config 缺少路径参数");
                        return 2;
                    }
                    configPath = args[++i];
                } else if (a.StartsWith("-")) {
                    Console.Error.WriteLine("未知参数: " + a + "。请使用 --help 查看用法。");
                    return 2;
                } else {
                    if (filePath != null) {
                        Console.Error.WriteLine("参数错误: 仅支持传入一个文件路径。");
                        return 2;
                    }
                    filePath = a;
                }
            }

            if (doHelp) {
                Console.WriteLine("MDReader 用法: mdreader.exe [<file.md>] [--config <config.json>] | --register | --unregister | --help");
                return 0;
            }
            if ((doRegister || doUnregister) && (!string.IsNullOrEmpty(filePath) || (doRegister && doUnregister))) {
                Console.Error.WriteLine("参数错误: --register/--unregister 不可与文件路径或彼此混用。");
                return 2;
            }
            if (doRegister) {
                AssocManager.Register(Application.ExecutablePath);
                return 0;
            }
            if (doUnregister) {
                AssocManager.Unregister();
                return 0;
            }

            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new MainForm(filePath, configPath));
            return 0;
        }
    }
}
'@

function Compile-MockAppExe {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$OutExePath
    )
    $parent = Split-Path -Parent $OutExePath
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    if (Test-Path -LiteralPath $OutExePath) {
        Remove-Item -LiteralPath $OutExePath -Force
    }
    Add-Type -TypeDefinition $script:MockAppCSharpSource `
        -OutputAssembly $OutExePath `
        -OutputType WindowsApplication `
        -ReferencedAssemblies @(
            "System.dll",
            "System.Drawing.dll",
            "System.Windows.Forms.dll",
            "System.Web.Extensions.dll"
        )
    if (-not (Test-Path -LiteralPath $OutExePath)) {
        throw "编译 MockApp 失败: 未生成 $OutExePath"
    }
    return (Resolve-Path -LiteralPath $OutExePath).Path
}

if ($CompileOnly -and $OutExePath) {
    $null = Compile-MockAppExe -OutExePath $OutExePath
}
