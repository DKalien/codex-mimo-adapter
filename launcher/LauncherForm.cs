using System.Diagnostics;
using System.Drawing;
using System.Net.Http;
using System.Net.Sockets;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace CodexMiMoLauncher;

internal sealed class LauncherForm : Form
{
    private const string DefaultHealthUrl = "http://127.0.0.1:4010/health";
    private static readonly string[] ManagedAgentFiles =
    {
        "default.toml",
        "explorer.toml",
        "worker.toml",
        "oss-worker-pro-1.toml",
        "oss-worker-pro-2.toml",
        "oss-worker-pro-3.toml",
        "oss-worker-std-1.toml",
        "oss-worker-std-2.toml",
        "oss-worker-std-3.toml",
    };
    private static readonly string[] LegacyAgentFiles =
    {
        "oss-flash.toml",
        "oss-mimo.toml",
        "oss-minimax.toml",
        "oss-pro.toml",
    };
    private readonly TextBox _apiKey = new() { UseSystemPasswordChar = true, Width = 242 };
    private readonly Label _status = new() { AutoSize = false, Height = 38, Dock = DockStyle.Fill, TextAlign = ContentAlignment.MiddleLeft };
    private readonly Label _runtime = new() { AutoSize = false, Height = 24, Dock = DockStyle.Fill, TextAlign = ContentAlignment.MiddleLeft, ForeColor = Color.DimGray };
    private readonly Button _start = new() { Text = "启动", Width = 64 };
    private readonly Button _check = new() { Text = "检查", Width = 64 };
    private readonly Button _stop = new() { Text = "关闭", Width = 64, Enabled = false };
    private readonly Button _restart = new() { Text = "重启", Width = 64, Enabled = false };
    private readonly Button _saveKey = new() { Text = "保存密钥", AutoSize = true };
    private readonly Button _clearKey = new() { Text = "清除", AutoSize = true };
    private readonly System.Windows.Forms.Timer _refreshTimer = new() { Interval = 3_000 };
    private readonly NotifyIcon _tray;
    private readonly LauncherPaths _paths;
    private readonly SecretStore _secrets;
    private readonly HttpClient _http = new() { Timeout = TimeSpan.FromSeconds(3) };
    private Process? _adapter;
    private RuntimeValidation _runtimeValidation;
    private bool _busy;
    private bool _exitRequested;
    // This is set before StopAsync reaches its first await. It prevents the Exited callback
    // from disposing the same Process object while StopAsync is still awaiting it.
    private Process? _stoppingAdapter;

    public LauncherForm()
    {
        _paths = LauncherPaths.Discover();
        _secrets = new SecretStore(_paths.UserDataDirectory);
        _runtimeValidation = _paths.ValidateRuntime();
        Text = "Codex MiMo";
        FormBorderStyle = FormBorderStyle.FixedToolWindow;
        StartPosition = FormStartPosition.Manual;
        Location = new Point(Math.Max(0, Screen.PrimaryScreen?.WorkingArea.Right - 340 ?? 0), 80);
        Size = new Size(332, 224);
        MinimumSize = Size;
        MaximumSize = Size;
        TopMost = true;
        ShowInTaskbar = false;

        var menu = new ContextMenuStrip();
        menu.Items.Add("显示窗口", null, (_, _) => RestoreFromTray());
        menu.Items.Add("检查状态", null, async (_, _) => await CheckAsync());
        menu.Items.Add(new ToolStripSeparator());
        menu.Items.Add("退出启动器", null, async (_, _) => await ExitLauncherAsync());
        _tray = new NotifyIcon
        {
            Icon = SystemIcons.Application,
            Text = "Codex MiMo Launcher",
            ContextMenuStrip = menu,
            Visible = true,
        };
        _tray.DoubleClick += (_, _) => RestoreFromTray();

        BuildLayout();
        _apiKey.Text = _secrets.Load() ?? string.Empty;
        _start.Click += async (_, _) => await StartAsync();
        _check.Click += async (_, _) => await CheckAsync();
        _stop.Click += async (_, _) => await StopAsync();
        _restart.Click += async (_, _) => await RestartAsync();
        _saveKey.Click += (_, _) => SaveKey();
        _clearKey.Click += (_, _) => ClearKey();
        _refreshTimer.Tick += async (_, _) => await RefreshStatusAsync();
        _refreshTimer.Start();
        Shown += async (_, _) => await RefreshStatusAsync();
        Resize += (_, _) =>
        {
            if (WindowState == FormWindowState.Minimized)
                Hide();
        };
        FormClosing += OnFormClosing;

        SetStatus("正在检查运行时…", Color.DimGray);
    }

    private void BuildLayout()
    {
        var table = new TableLayoutPanel
        {
            Dock = DockStyle.Fill,
            Padding = new Padding(10),
            ColumnCount = 1,
            RowCount = 5,
        };
        table.RowStyles.Add(new RowStyle(SizeType.Absolute, 29));
        table.RowStyles.Add(new RowStyle(SizeType.Absolute, 32));
        table.RowStyles.Add(new RowStyle(SizeType.Absolute, 36));
        table.RowStyles.Add(new RowStyle(SizeType.Absolute, 36));
        table.RowStyles.Add(new RowStyle(SizeType.Percent, 100));

        var keyLine = new FlowLayoutPanel { Dock = DockStyle.Fill, FlowDirection = FlowDirection.LeftToRight, WrapContents = false };
        keyLine.Controls.Add(new Label { Text = "MiMo API Key", AutoSize = true, Margin = new Padding(0, 6, 8, 0) });
        keyLine.Controls.Add(_apiKey);
        table.Controls.Add(keyLine, 0, 0);

        var keyButtons = new FlowLayoutPanel { Dock = DockStyle.Fill, FlowDirection = FlowDirection.LeftToRight, WrapContents = false };
        keyButtons.Controls.Add(_saveKey);
        keyButtons.Controls.Add(_clearKey);
        table.Controls.Add(keyButtons, 0, 1);

        var actions = new FlowLayoutPanel { Dock = DockStyle.Fill, FlowDirection = FlowDirection.LeftToRight, WrapContents = false };
        actions.Controls.AddRange(new Control[] { _start, _check, _stop, _restart });
        table.Controls.Add(actions, 0, 2);
        table.Controls.Add(_status, 0, 3);
        table.Controls.Add(_runtime, 0, 4);
        Controls.Add(table);
    }

    private async Task StartAsync()
    {
        if (_busy) return;
        var apiKey = _apiKey.Text.Trim();
        if (string.IsNullOrWhiteSpace(apiKey))
        {
            SetStatus("需要先输入并保存 MiMo API Key。", Color.Firebrick);
            _apiKey.Focus();
            return;
        }
        if (!EnsureRuntimeValidated()) return;
        if (_adapter is { HasExited: false })
        {
            SetStatus("适配器已由此启动器启动。", Color.DarkGoldenrod);
            return;
        }
        if (await IsHealthyAsync())
        {
            SetStatus("端口 4010 已有可用适配器；不会接管外部进程。", Color.DarkGoldenrod);
            return;
        }
        if (await IsPortOccupiedAsync())
        {
            SetStatus("端口 4010 被其他程序占用，无法启动。", Color.Firebrick);
            return;
        }

        SetBusy(true);
        try
        {
            SaveKey();
            var initializationStatus = GetInitializationStatus();
            if (initializationStatus is not null)
            {
                SetStatus(initializationStatus, Color.DimGray);
                var initialized = await RunInitializationAsync(apiKey);
                if (!initialized) return;
            }
            if (!TryConfigureCodexAuthCommand(out var configurationError))
            {
                SetStatus($"Codex 鉴权配置失败：{configurationError}", Color.Firebrick);
                return;
            }

            var startInfo = CreateCoreStartInfo("run");
            startInfo.Environment["MIMO_API_KEY"] = apiKey;
            var process = new Process { StartInfo = startInfo, EnableRaisingEvents = true };
            process.OutputDataReceived += (_, e) => AppendLog("core", e.Data, null, _apiKey.Text.Trim());
            process.ErrorDataReceived += (_, e) => AppendLog("core", null, e.Data, _apiKey.Text.Trim());
            process.Exited += AdapterExited;
            if (!process.Start())
            {
                SetStatus("无法启动核心适配器进程。", Color.Firebrick);
                return;
            }
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();
            _adapter = process;
            _stop.Enabled = _restart.Enabled = true;
            SetStatus("正在启动适配器…", Color.DimGray);
            await WaitForHealthyAsync();
        }
        catch (Exception ex)
        {
            SetStatus($"启动失败：{ReadableError(ex)}", Color.Firebrick);
        }
        finally
        {
            SetBusy(false);
        }
    }

    private string? GetInitializationStatus()
    {
        if (!File.Exists(_paths.ProjectEnvironmentFile))
            return "首次使用：正在初始化项目配置…";

        var agentsDirectory = Path.Combine(_paths.RepositoryRoot.FullName, ".codex", "agents");
        var missingManagedFiles = ManagedAgentFiles
            .Where(fileName => !File.Exists(Path.Combine(agentsDirectory, fileName)))
            .ToArray();
        var legacyFiles = LegacyAgentFiles
            .Where(fileName => File.Exists(Path.Combine(agentsDirectory, fileName)))
            .ToArray();

        if (missingManagedFiles.Length == 0 && legacyFiles.Length == 0)
            return null;

        var details = new List<string>();
        if (missingManagedFiles.Length > 0)
            details.Add($"缺少 {missingManagedFiles.Length} 个九模板配置");
        if (legacyFiles.Length > 0)
            details.Add($"检测到 {legacyFiles.Length} 个旧版配置");
        return $"正在升级子代理配置（{string.Join("；", details)}）…";
    }

    private async Task<bool> RunInitializationAsync(string apiKey)
    {
        var startInfo = CreateCoreStartInfo("init", "--api-key-stdin");
        using var process = new Process { StartInfo = startInfo };
        try
        {
            if (!process.Start())
            {
                SetStatus("无法启动核心初始化程序。", Color.Firebrick);
                return false;
            }
            await process.StandardInput.WriteLineAsync(apiKey);
            process.StandardInput.Close();
            var stdout = await process.StandardOutput.ReadToEndAsync();
            var stderr = await process.StandardError.ReadToEndAsync();
            await process.WaitForExitAsync();
            if (process.ExitCode != 0)
            {
                AppendLog("init", stdout, stderr, apiKey);
                SetStatus("初始化失败。请点击检查，或查看启动器日志。", Color.Firebrick);
                return false;
            }
            AppendLog("init", stdout, stderr, apiKey);
            return true;
        }
        catch (Exception ex)
        {
            SetStatus($"初始化异常：{ReadableError(ex)}", Color.Firebrick);
            return false;
        }
    }

    private async Task<bool> StopAsync()
    {
        var adapter = _adapter;
        if (adapter is null)
        {
            _stop.Enabled = _restart.Enabled = false;
            await RefreshStatusAsync();
            return true;
        }
        if (adapter.HasExited)
        {
            ReleaseAdapter(adapter);
            await RefreshStatusAsync();
            return true;
        }

        SetBusy(true);
        _stoppingAdapter = adapter;
        try
        {
            SetStatus("正在关闭由启动器管理的适配器…", Color.DimGray);
            try
            {
                adapter.Kill(entireProcessTree: true);
            }
            catch (InvalidOperationException) when (adapter.HasExited)
            {
                // The core exited between the initial check and Kill; it is already stopped.
            }
            await adapter.WaitForExitAsync();
            SetStatus("适配器已关闭。", Color.DimGray);
            return true;
        }
        catch (Exception ex)
        {
            SetStatus($"关闭失败：{ReadableError(ex)}", Color.Firebrick);
            return false;
        }
        finally
        {
            if (HasExited(adapter))
                ReleaseAdapter(adapter);
            else if (ReferenceEquals(_stoppingAdapter, adapter))
                _stoppingAdapter = null;
            SetBusy(false);
        }
    }

    private async Task RestartAsync()
    {
        if (await StopAsync())
            await StartAsync();
    }

    private async Task CheckAsync()
    {
        if (!EnsureRuntimeValidated()) return;
        SetBusy(true);
        try
        {
            await RefreshStatusAsync();
            if (!await IsHealthyAsync() && !await IsPortOccupiedAsync())
                SetStatus("核心文件就绪，但适配器未运行。", Color.DimGray);
        }
        finally
        {
            SetBusy(false);
        }
    }

    private async Task RefreshStatusAsync()
    {
        _runtime.Text = _runtimeValidation.Description;
        if (await IsHealthyAsync())
        {
            SetStatus(_adapter is { HasExited: false } ? "运行中（由此启动器管理）。" : "运行中（外部进程）。", Color.ForestGreen);
            return;
        }
        if (_adapter is { HasExited: false })
        {
            SetStatus("启动中或健康检查未通过…", Color.DarkGoldenrod);
            return;
        }
        _stop.Enabled = _restart.Enabled = false;
        if (_runtimeValidation.IsValid)
            SetStatus("已就绪，等待启动。", Color.DimGray);
        else
            SetStatus(_runtimeValidation.Description, Color.Firebrick);
    }

    private async Task WaitForHealthyAsync()
    {
        for (var attempt = 0; attempt < 15; attempt++)
        {
            if (await IsHealthyAsync())
            {
                SetStatus("运行中（由此启动器管理）。", Color.ForestGreen);
                return;
            }
            if (_adapter is null || _adapter.HasExited)
            {
                SetStatus("核心进程意外退出。请查看启动器日志。", Color.Firebrick);
                return;
            }
            await Task.Delay(400);
        }
        SetStatus("核心已启动，但 4010 健康检查尚未通过。", Color.DarkGoldenrod);
    }

    private async Task<bool> IsHealthyAsync()
    {
        try
        {
            using var response = await _http.GetAsync(DefaultHealthUrl);
            return response.IsSuccessStatusCode;
        }
        catch (HttpRequestException) { return false; }
        catch (TaskCanceledException) { return false; }
    }

    private static async Task<bool> IsPortOccupiedAsync()
    {
        try
        {
            using var client = new TcpClient();
            await client.ConnectAsync("127.0.0.1", 4010);
            return true;
        }
        catch (SocketException) { return false; }
    }

    private ProcessStartInfo CreateCoreStartInfo(params string[] arguments)
    {
        var info = new ProcessStartInfo
        {
            FileName = _runtimeValidation.CoreExecutable?.FullName ?? _paths.CoreExecutable.FullName,
            WorkingDirectory = _paths.RepositoryRoot.FullName,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        foreach (var argument in arguments) info.ArgumentList.Add(argument);
        return info;
    }

    private bool EnsureRuntimeValidated()
    {
        _runtimeValidation = _paths.ValidateRuntime();
        _runtime.Text = _runtimeValidation.Description;
        if (_runtimeValidation.IsValid) return true;
        SetStatus(_runtimeValidation.Description, Color.Firebrick);
        return false;
    }

    private bool TryConfigureCodexAuthCommand(out string error)
    {
        error = string.Empty;
        var configPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".codex", "config.toml");
        try
        {
            if (!File.Exists(configPath))
            {
                error = $"未找到 {configPath}";
                return false;
            }
            var source = File.ReadAllText(configPath);
            const string sectionPattern = @"(?ms)(^\s*\[model_providers\.mimo_adapter\.auth\]\s*\r?\n)(.*?)(?=^\s*\[|\z)";
            var match = Regex.Match(source, sectionPattern);
            if (!match.Success)
            {
                error = "未找到 [model_providers.mimo_adapter.auth] 配置节";
                return false;
            }
            var corePath = _runtimeValidation.CoreExecutable?.FullName ?? _paths.CoreExecutable.FullName;
            var escapedPath = corePath.Replace("\\", "\\\\").Replace("\"", "\\\"");
            var commandLine = $"command = \"{escapedPath}\"";
            var body = match.Groups[2].Value;
            var updatedBody = Regex.IsMatch(body, @"(?m)^\s*command\s*=")
                ? Regex.Replace(body, @"(?m)^\s*command\s*=.*$", commandLine)
                : commandLine + Environment.NewLine + body;
            var updated = source[..match.Index] + match.Groups[1].Value + updatedBody + source[(match.Index + match.Length)..];
            if (String.Equals(source, updated, StringComparison.Ordinal)) return true;

            var backupPath = configPath + $".launcher.bak.{DateTimeOffset.Now:yyyyMMddHHmmss}";
            File.Copy(configPath, backupPath, overwrite: false);
            var temporaryPath = configPath + ".launcher.tmp";
            File.WriteAllText(temporaryPath, updated);
            File.Move(temporaryPath, configPath, overwrite: true);
            AppendLog("config", $"Updated mimo_adapter auth.command in {configPath}", null, null);
            return true;
        }
        catch (Exception ex)
        {
            error = ReadableError(ex);
            return false;
        }
    }

    private void AdapterExited(object? sender, EventArgs e)
    {
        if (sender is not Process exited || IsDisposed || Disposing || !IsHandleCreated) return;
        try
        {
            BeginInvoke(() => HandleAdapterExited(exited));
        }
        catch (InvalidOperationException)
        {
            // The window was disposed after the checks above.
        }
    }

    private void HandleAdapterExited(Process exited)
    {
        if (!ReferenceEquals(_adapter, exited) || ReferenceEquals(_stoppingAdapter, exited)) return;
        ReleaseAdapter(exited);
        SetStatus("核心进程已退出。", Color.Firebrick);
    }

    private void ReleaseAdapter(Process adapter)
    {
        if (ReferenceEquals(_adapter, adapter)) _adapter = null;
        if (ReferenceEquals(_stoppingAdapter, adapter)) _stoppingAdapter = null;
        adapter.Exited -= AdapterExited;
        adapter.Dispose();
        _stop.Enabled = _restart.Enabled = false;
    }

    private static bool HasExited(Process process)
    {
        try
        {
            return process.HasExited;
        }
        catch (InvalidOperationException)
        {
            return true;
        }
    }

    private void SaveKey()
    {
        var value = _apiKey.Text.Trim();
        if (string.IsNullOrEmpty(value))
        {
            SetStatus("没有可保存的 API Key。", Color.Firebrick);
            return;
        }
        try
        {
            _secrets.Save(value);
            SetStatus("API Key 已使用 Windows 当前用户保护保存。", Color.DimGray);
        }
        catch (Exception ex)
        {
            SetStatus($"无法保存 API Key：{ReadableError(ex)}", Color.Firebrick);
        }
    }

    private void ClearKey()
    {
        _apiKey.Clear();
        _secrets.Clear();
        SetStatus("已清除启动器保存的 API Key。", Color.DimGray);
    }

    private void SetBusy(bool busy)
    {
        _busy = busy;
        _start.Enabled = _check.Enabled = _saveKey.Enabled = _clearKey.Enabled = !busy;
        _stop.Enabled = !busy && _adapter is { HasExited: false };
        _restart.Enabled = !busy && _adapter is { HasExited: false };
        UseWaitCursor = busy;
    }

    private void SetStatus(string message, Color color)
    {
        _status.Text = message;
        _status.ForeColor = color;
        _tray.Text = message.Length > 63 ? "Codex MiMo Launcher" : message;
    }

    private void AppendLog(string category, string? stdout, string? stderr, string? secret)
    {
        try
        {
            var text = string.Join(Environment.NewLine, new[] { stdout, stderr }.Where(value => !string.IsNullOrWhiteSpace(value)));
            if (string.IsNullOrWhiteSpace(text)) return;
            if (!string.IsNullOrEmpty(secret)) text = text.Replace(secret, "***", StringComparison.Ordinal);
            Directory.CreateDirectory(_paths.UserDataDirectory);
            File.AppendAllText(_paths.LogFile, $"{DateTimeOffset.Now:O} [{category}] {text}{Environment.NewLine}");
        }
        catch { /* Logging must not prevent launcher use. */ }
    }

    private async Task ExitLauncherAsync()
    {
        if (_exitRequested) return;
        _exitRequested = true;
        _refreshTimer.Stop();
        if (!await StopAsync())
        {
            _exitRequested = false;
            _refreshTimer.Start();
            return;
        }
        Close();
    }

    private void RestoreFromTray()
    {
        Show();
        WindowState = FormWindowState.Normal;
        Activate();
    }

    internal void ActivateFromAnotherInstance()
    {
        if (IsDisposed || Disposing || !IsHandleCreated) return;
        try
        {
            BeginInvoke((Action)RestoreFromTray);
        }
        catch (InvalidOperationException)
        {
            // The primary instance is shutting down; the second instance will exit normally.
        }
    }

    private void OnFormClosing(object? sender, FormClosingEventArgs e)
    {
        if (_exitRequested) return;
        e.Cancel = true;
        Hide();
        _tray.ShowBalloonTip(1200, "Codex MiMo", "启动器仍在托盘中运行。", ToolTipIcon.Info);
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            _refreshTimer.Dispose();
            _tray.Dispose();
            _http.Dispose();
            if (_adapter is { } adapter)
                ReleaseAdapter(adapter);
        }
        base.Dispose(disposing);
    }

    private static string ReadableError(Exception exception) => exception.Message.Replace(Environment.NewLine, " ", StringComparison.Ordinal).Trim();
}

internal sealed class LauncherPaths
{
    public const string LauncherVersion = "0.1.0";
    private const string RuntimeRelativePath = "runtime\\windows-x64\\codex-mimo-adapter.exe";
    public DirectoryInfo RepositoryRoot { get; }
    public DirectoryInfo RuntimeDirectory => new(Path.Combine(RepositoryRoot.FullName, "runtime", "windows-x64"));
    public FileInfo CoreExecutable => new(Path.Combine(RepositoryRoot.FullName, RuntimeRelativePath));
    public string ProjectEnvironmentFile => Path.Combine(RepositoryRoot.FullName, ".codex-mimo-adapter.env");
    public string UserDataDirectory { get; } = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexMiMoLauncher");
    public string LogFile => Path.Combine(UserDataDirectory, "launcher.log");

    private LauncherPaths(DirectoryInfo root) => RepositoryRoot = root;

    public static LauncherPaths Discover()
    {
        var candidates = new[] { Environment.CurrentDirectory, AppContext.BaseDirectory };
        foreach (var candidate in candidates)
        {
            var directory = new DirectoryInfo(candidate);
            while (directory is not null)
            {
                if (File.Exists(Path.Combine(directory.FullName, RuntimeRelativePath)) || File.Exists(Path.Combine(directory.FullName, ".git", "HEAD")))
                    return new LauncherPaths(directory);
                directory = directory.Parent;
            }
        }
        // Keep the expected location visible when a first-time clone has no runtime yet.
        return new LauncherPaths(new DirectoryInfo(Environment.CurrentDirectory));
    }

    public RuntimeValidation ValidateRuntime()
    {
        var manifest = Path.Combine(RuntimeDirectory.FullName, "manifest.json");
        if (!File.Exists(manifest))
            return RuntimeValidation.Invalid("核心运行时尚未安装：缺少 runtime\\windows-x64\\manifest.json。", missing: true);
        try
        {
            using var document = JsonDocument.Parse(File.ReadAllText(manifest));
            var root = document.RootElement;
            if (!root.TryGetProperty("schema_version", out var schema) || schema.ValueKind != JsonValueKind.Number || !schema.TryGetInt32(out var schemaVersion) || schemaVersion != 1)
                return RuntimeValidation.Invalid("核心运行时 manifest 的 schema_version 无效。");
            if (!root.TryGetProperty("platform", out var platform) || platform.ValueKind != JsonValueKind.String || platform.GetString() != "windows-x64")
                return RuntimeValidation.Invalid("核心运行时不是 windows-x64 平台。");
            if (!root.TryGetProperty("adapter", out var adapter) || adapter.ValueKind != JsonValueKind.Object)
                return RuntimeValidation.Invalid("核心运行时 manifest 缺少 adapter 信息。");
            if (!adapter.TryGetProperty("file", out var fileName) || fileName.ValueKind != JsonValueKind.String || String.IsNullOrWhiteSpace(fileName.GetString()))
                return RuntimeValidation.Invalid("核心运行时 manifest 缺少 adapter.file。");
            var relativeFile = fileName.GetString()!;
            if (Path.IsPathRooted(relativeFile) || relativeFile.IndexOfAny(Path.GetInvalidPathChars()) >= 0)
                return RuntimeValidation.Invalid("核心运行时 manifest 的 adapter.file 不安全。");
            var runtimeRoot = Path.GetFullPath(RuntimeDirectory.FullName + Path.DirectorySeparatorChar);
            var adapterPath = Path.GetFullPath(Path.Combine(RuntimeDirectory.FullName, relativeFile));
            if (!adapterPath.StartsWith(runtimeRoot, StringComparison.OrdinalIgnoreCase) || !String.Equals(Path.GetExtension(adapterPath), ".exe", StringComparison.OrdinalIgnoreCase))
                return RuntimeValidation.Invalid("核心运行时 manifest 的 adapter.file 不在 runtime 目录内。");
            var executable = new FileInfo(adapterPath);
            if (!executable.Exists)
                return RuntimeValidation.Invalid($"核心运行时缺少 adapter 文件：{relativeFile}。", missing: true);
            if (!adapter.TryGetProperty("sha256", out var expectedHash) || expectedHash.ValueKind != JsonValueKind.String || !IsSha256(expectedHash.GetString()))
                return RuntimeValidation.Invalid("核心运行时 manifest 的 adapter.sha256 无效。");
            var actualHash = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(File.ReadAllBytes(executable.FullName))).ToLowerInvariant();
            if (!String.Equals(actualHash, expectedHash.GetString(), StringComparison.OrdinalIgnoreCase))
                return RuntimeValidation.Invalid("核心运行时校验失败：adapter SHA-256 不匹配。请重新获取 runtime。");
            if (!root.TryGetProperty("minimum_launcher_version", out var minimumVersion) || minimumVersion.ValueKind != JsonValueKind.String || !SemanticVersion.TryParse(minimumVersion.GetString(), out var required))
                return RuntimeValidation.Invalid("核心运行时 manifest 的 minimum_launcher_version 无效。");
            if (!SemanticVersion.TryParse(LauncherVersion, out var current) || current.CompareTo(required) < 0)
                return RuntimeValidation.Invalid($"核心运行时要求启动器 {minimumVersion.GetString()} 或更高版本（当前 {LauncherVersion}）。");
            var adapterVersion = adapter.TryGetProperty("version", out var version) ? version.GetString() : null;
            return RuntimeValidation.Valid(executable, $"核心：{adapterVersion ?? "已就绪"}（已校验）");
        }
        catch (JsonException) { return RuntimeValidation.Invalid("核心运行时 manifest 不是有效 JSON。"); }
        catch (IOException ex) { return RuntimeValidation.Invalid($"无法读取核心运行时：{ex.Message}"); }
    }

    private static bool IsSha256(string? value) => value is not null && Regex.IsMatch(value, "^[0-9a-fA-F]{64}$");
}

internal sealed record RuntimeValidation(bool IsValid, bool IsMissing, FileInfo? CoreExecutable, string Description)
{
    public static RuntimeValidation Valid(FileInfo coreExecutable, string description) => new(true, false, coreExecutable, description);
    public static RuntimeValidation Invalid(string description, bool missing = false) => new(false, missing, null, description);
}

internal readonly record struct SemanticVersion(int Major, int Minor, int Patch, string? PreRelease) : IComparable<SemanticVersion>
{
    private static readonly Regex Pattern = new("^(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)(?:-([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?$", RegexOptions.CultureInvariant);

    public static bool TryParse(string? value, out SemanticVersion version)
    {
        var match = value is null ? Match.Empty : Pattern.Match(value);
        if (!match.Success || !Int32.TryParse(match.Groups[1].Value, out var major) || !Int32.TryParse(match.Groups[2].Value, out var minor) || !Int32.TryParse(match.Groups[3].Value, out var patch))
        {
            version = default;
            return false;
        }
        version = new SemanticVersion(major, minor, patch, match.Groups[4].Success ? match.Groups[4].Value : null);
        return true;
    }

    public int CompareTo(SemanticVersion other)
    {
        var core = Major.CompareTo(other.Major);
        if (core != 0) return core;
        core = Minor.CompareTo(other.Minor);
        if (core != 0) return core;
        core = Patch.CompareTo(other.Patch);
        if (core != 0) return core;
        if (PreRelease is null) return other.PreRelease is null ? 0 : 1;
        if (other.PreRelease is null) return -1;
        return StringComparer.Ordinal.Compare(PreRelease, other.PreRelease);
    }
}
