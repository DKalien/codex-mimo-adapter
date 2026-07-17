# Windows 图形启动器部署

本页有两条完全分开的路径：普通 Windows 用户只看下一节的六步；维护此项目并发布新版本的人请看后面的“发布者”一节。旧的 PowerShell 便携包流程在文末，不能与图形启动器混用。

## 普通 Windows 用户：只需六步

**开始前先确认：发布者必须已经实际运行过 `Windows Runtime` 工作流，或已经提供组合 Release 包。** 当前仓库的源代码本身不能直接使用；若还没有组合包，请联系发布者生成第一份包。

1. 从发布者提供的 GitHub Release，或 `Windows Runtime` 工作流的 Artifacts，下载名称类似 `codex-mimo-adapter-windows-x64-<提交号>` 的**组合包**。不要下载名称含 `core-windows-x64` 的核心运行时包。
2. 将组合包解压到任意本地目录；不要只把其中一个 EXE 单独复制出来。
3. 双击解压根目录的 `CodexMiMoLauncher.exe`。它会显示一个置顶的小窗口，也会在系统托盘保留图标。
4. 在 `MiMo API Key` 输入框粘贴你的 MiMo Token Plan API Key。
5. 点击“保存密钥”。
6. 点击“启动”，状态显示“运行中”后，重启 Codex Desktop；之后保持启动器运行。需要确认时点“检查”，需要停止或再次启动时点“关闭”或“重启”。关闭窗口只会最小化到托盘；从托盘菜单选择“退出启动器”才会退出。

这条路径不需要安装 Rust、.NET SDK、PowerShell 或独立 Codex CLI。也**不能**用裸 `git clone`，更不能只下载 core-only runtime artifact：启动器、核心适配器和它们的 manifest 必须保持在同一个解压根目录内。

### 开发设备：无感切换到 Release 核心

如果此 Windows 用户已有一个可用的单项目适配器配置，Launcher 的“启动”会优先复用它：不会写入 Release 解压目录、不会添加项目注册表项、不会改写 `%USERPROFILE%\.codex\config.toml`，也不会保存或读取启动器自己的 API Key。它只用 Release 核心 EXE 启动已注册项目，因此可在停止开发实例后无感切换。

共享启动要求现有注册表恰好有一个有效项目，项目环境文件中已有 API Key 和本地 Token，且 Provider 仍指向 `http://127.0.0.1:4010/v1`。开发设备必须使用未初始化的全新 Release 解压目录；检测到多项目、残留注册表或不兼容 Provider 时，Launcher 会拒绝启动且不自动回退到初始化，以避免修改现有配置。没有现有适配器配置的普通用户仍走上面的首次初始化流程。

共享预检只验证非敏感的结构兼容性；真正的可用性仍以启动后的健康检查为准。

### 使用前应知道的安全与完整性保护

- 启动器用当前 Windows 用户的 DPAPI 保存 API Key，保存位置在用户 LocalAppData 目录下、项目仓库之外；密钥不作为命令行参数传递，也不会写入项目配置文件。
- 每次启动前，启动器都会读取 `runtime/windows-x64/manifest.json`，检查平台、最低启动器版本和核心 EXE 的 SHA-256。检查失败时不要强行运行；重新获取完整组合包。
- 适配器仅在本机 `127.0.0.1:4010` 提供服务。启动器不会接管已由其他程序启动的适配器。

### 普通用户故障排查

| 现象 | 处理方式 |
|---|---|
| 双击后提示缺少运行时或 manifest | 重新下载并完整解压**组合包**；确认 EXE 下方存在 `runtime/windows-x64/codex-mimo-adapter.exe` 和 `manifest.json`。 |
| 提示 SHA-256 或版本校验失败 | 删除当前解压目录后重新获取组合包；不要混用不同版本的启动器和运行时。 |
| “端口 4010 被其他程序占用” | 先退出占用该端口的程序，或回到已有适配器的启动器管理它；本启动器不会关闭外部进程。 |
| 点击启动后仍未运行 | 点“检查”并查看状态；确认 API Key 已保存且有效。仍失败时保留启动器日志并联系发布者。 |
| Codex Desktop 没有使用适配器 | 在启动器显示“运行中”后完全退出并重新打开 Codex Desktop。 |

## 发布者：构建并提供组合包

本节只适用于仓库维护者或发布者，不是普通用户的安装步骤。

### 在 CI 中生成包（推荐）

1. 提交并推送需要发布的代码。
2. 若只需 Artifact，可在 GitHub Actions 手动运行 `Windows Runtime` 工作流；手动运行不会创建 GitHub Release。若需发布，请推送与 `Cargo.toml` 中 `version` 完全匹配的 `v<version>` 标签，例如 `v0.2.2`。
3. 等待 `Build Windows x64 runtime` 成功。
4. 所有运行都会上传 artifact `codex-mimo-adapter-windows-x64-<提交号>`。标签运行还会创建或更新对应的 GitHub Release，并上传 `codex-mimo-adapter-windows-x64-v<version>.zip`。
5. Release ZIP 保留 `codex-mimo-adapter-windows-x64/` 根目录；解压后从该目录运行 `CodexMiMoLauncher.exe`。

CI 使用 Windows runner、Rust 和 .NET 8 SDK 构建：核心 Rust 适配器、单文件且自包含的 `CodexMiMoLauncher.exe`、以及带 SHA-256 的 runtime manifest。发布前应解压一次组合包并按前述六步做一次冒烟验证。

### 本地开发构建

本地构建只面向开发者：需要 Rust（含 `x86_64-pc-windows-msvc` 目标）和 .NET 8 SDK。开发者可使用仓库脚本生成核心运行时与 manifest，再发布自包含 Launcher；详细构建参数以 CI 工作流和英文文档为准。不要把生成的 EXE、manifest 或任何 API Key 提交到仓库。

## 旧版 CLI/便携包（非普通用户路径）

旧包使用 `.env` 和 `start-portable.ps1` 进行配置与启动。这条路径只供需要维护旧部署的技术用户使用；它不是图形启动器的前置步骤，也不要与组合包混用。旧版的完整命令、配置和迁移说明请见[英文 PORTABLE.md](PORTABLE.md)。
