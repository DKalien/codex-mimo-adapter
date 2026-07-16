# 便携式部署

本文档说明如何使用预构建的便携式 ZIP 包，在无需 Rust 工具链的 Windows x64
计算机上部署 `codex-mimo-adapter`。

## Launcher 运行时更新

便携式 ZIP 包仍然受支持。桌面 Launcher 发行版改用独立的 Windows x64 核心运行时：
`codex-mimo-adapter.exe` 加上 `runtime/windows-x64/manifest.json`。该 manifest 记录
适配器版本、SHA-256、平台和最低兼容 Launcher 版本。

开发机和 CI 使用以下命令创建该运行时：

```powershell
.\scripts\stage-runtime-windows.ps1 -MinimumLauncherVersion "0.1.0"
```

可执行文件和生成的 manifest 被刻意加入 Git 忽略规则。请勿将它们或 API Key 提交到
仓库。CI 会上传纯核心运行时 artifact 和面向最终用户的组合 artifact。组合 artifact
的结构如下：

```text
CodexMiMoLauncher.exe
runtime/windows-x64/codex-mimo-adapter.exe
runtime/windows-x64/manifest.json
```

当前 Launcher 要求在同一个解压根目录下存在与其匹配的运行时；它不会为仅包含源代码的
裸克隆下载运行时。manifest 是运行时包的完整性约定。

## 选择合适的安装方式

- **开发机：** 克隆源代码仓库，并安装 Rust 以构建核心运行时。构建自包含的 Windows
  Launcher 还需要 .NET 8 SDK。使用 `stage-runtime-windows.ps1` 将本地核心放入预期的
  运行时目录。
- **普通 Windows 用户：** 下载并解压 CI 的 Windows x64 组合 artifact（或未来的组合
  Release 包），然后从解压根目录运行 `CodexMiMoLauncher.exe`。不要将裸 `git clone`
  或纯核心运行时 artifact 视为可直接运行的安装包。

Launcher 会使用当前 Windows 用户的 DPAPI 配置文件，在仓库之外保存输入的 MiMo API Key。
首次运行时，它会调用 `codex-mimo-adapter init --api-key-stdin`：密钥通过标准输入传递，
项目配置仅记录该密钥来自进程环境，之后启动的核心会继承这个环境变量值。

## 包内容

```
codex-mimo-adapter-{version}-windows-x64/
  codex-mimo-adapter.exe    # 适配器二进制文件
  .env.example              # 环境变量模板
  config.toml.example       # Codex 提供商配置模板
  agents/                   # 全局子代理模板（首次启动时重写路由）
  scripts/
    start-portable.ps1      # 一键启动脚本
    check-local-adapter.ps1 # 健康检查脚本
  PORTABLE.md               # 本文件
  README.md                 # 项目 README
  NOTICE                    # 第三方声明
```

## 前置条件

- Windows 10/11 x64
- PowerShell 5.1+（随 Windows 提供）
- 一个 MiMo Token Plan API Key

## 快速开始

1. **解压** ZIP 压缩包至任意目录。

2. 通过编辑 `.env`（或从 `.env.example` 复制）来**配置**适配器：
   ```powershell
   # 设置 API Key
   MIMO_API_KEY=your-mimo-token-plan-api-key
   ```

3. **启动**适配器：
   ```powershell
   .\scripts\start-portable.ps1 -ApiKey "your-mimo-token-plan-api-key"
   ```

   后续运行时（当 `.codex-mimo-adapter.env` 已存在）：
   ```powershell
   .\scripts\start-portable.ps1 -NoInit
   ```

   首次运行还会将已路由的代理定义安装到 `%USERPROFILE%\.codex\agents`，使当前
   Windows 用户的每个 Codex 项目都能使用它们。它会将包内 EXE 的绝对路径写入提供商
   身份验证辅助程序，因此无需安装独立 CLI 或更改 PATH。首次运行后请重启 Codex Desktop。

   如需在不立即启动服务的情况下配置目标计算机：
   ```powershell
   .\scripts\start-portable.ps1 -ApiKey "your-mimo-token-plan-api-key" -ConfigureOnly
   ```

4. **验证**其是否正常工作：
   ```powershell
   .\scripts\check-local-adapter.ps1
   ```

## 配置

### 环境变量（.env）

| 变量 | 默认值 | 说明 |
|---|---|---|
| `MIMO_API_KEY` | （必需） | 你的 MiMo Token Plan API Key |
| `MIMO_API_BASE_URL` | `https://token-plan-cn.xiaomimimo.com/v1` | 上游 API 基础 URL |
| `CODEX_MIMO_LOCAL_TOKEN` | `codex-mimo-local` | 适配器的本地认证令牌 |
| `CODEX_MIMO_PROJECT_ID` | `mimo_adapter_example` | 项目标识符 |
| `CODEX_MIMO_HOST` | `127.0.0.1` | 监听地址 |
| `CODEX_MIMO_PORT` | `4010` | 监听端口 |
| `CODEX_MIMO_STATE_DB` | `.codex-mimo/state.sqlite` | 状态数据库路径 |

### Codex 提供商配置（config.toml.example）

将 `config.toml.example` 的内容合并到用户级 Codex 配置中：

- **Windows**：`%USERPROFILE%\.codex\config.toml`
- **macOS/Linux**：`~/.codex/config.toml`

这会注册供 Codex 使用的 `mimo_adapter` 提供商。

## 与 Codex 集成

启动适配器后，配置 Codex 通过它进行路由：

```powershell
# 将 mimo_adapter 提供商添加到全局 Codex 配置
codex-mimo-adapter init --api-key "your-key"
```

或者，将 `config.toml.example` 中的提供商配置块手动添加至
`%USERPROFILE%\.codex\config.toml`。仍建议使用启动脚本，因为它还会安装全局路由的
子代理定义。

## 故障排除

### 适配器无法启动

- 确保没有其他进程正在使用端口 4010。
- 检查 `.env` 是否包含有效的 `MIMO_API_KEY`。

### 健康检查失败

- 确认适配器正在另一个终端中运行。
- 验证端口是否匹配（默认值：4010）。

### 模型列表为空

- 运行 `codex-mimo-adapter init` 来设置项目注册表。

## 卸载

直接删除解压后的目录即可。适配器是自包含的，不会写入 Windows 注册表或 Program Files。
