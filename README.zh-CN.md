# Codex MiMo Token Plan 适配器

一个轻量级适配器，用于连接 Codex 子代理使用的 OpenAI Responses API 和 MiMo Token Plan Chat Completions API。

它只做协议转换，不运行额外的代理系统。

## 数据流

```text
Codex 子代理
  -> POST /v1/responses (Responses API)
  -> 本适配器
  -> POST /chat/completions (Chat Completions API)
  -> MiMo Token Plan (https://token-plan-cn.xiaomimimo.com/v1)
  -> Chat Completions 响应 / SSE 流
  -> 本适配器
  -> Responses API 响应 / SSE 流
  -> Codex 子代理
```

Codex 负责任务角色分配、沙箱权限、工具执行、审查以及判断任务是否完成。

## 安装

本项目尚未发布到 `crates.io`。

将当前本地代码安装为全局 Cargo CLI：

```powershell
cd D:\AI-Tools\codex-mimo-adapter
cargo install --path .
```

如果更新了本地源码并想重新安装：

```powershell
cargo install --path . --force
```

安装完成后，`codex-mimo-adapter` 将作为全局命令可用。

## 快速开始

初始化项目、启动适配器并验证：

```powershell
# 1. 使用 MiMo Token Plan API 密钥初始化当前项目
codex-mimo-adapter init --api-key "<你的-mimo-token-plan-api-key>"
#   - 写入 .codex-mimo-adapter.env（项目级环境变量）
#   - 在全局注册表中注册项目（~/.codex-mimo-adapter/）
#   - 写入 .codex/agents/*.toml，包含路由后的模型名称
#   - 写入 ~/.codex/config.toml，包含单个 "mimo_adapter" provider

# 2. 启动适配器（单实例服务所有已注册项目）
codex-mimo-adapter run

# 3. 在另一个终端中验证适配器健康状态
codex-mimo-adapter check

# 4. 打印签名的本地令牌（用于 Codex provider auth 命令）
codex-mimo-adapter auth print-local-token
```

开发阶段可以直接从源码运行：

```powershell
cargo run -- init --api-key "<你的密钥>"
cargo run -- run
```

或使用仓库内的开发脚本：

```powershell
./scripts/dev-run.ps1 -ApiKey "<你的密钥>"
```

### Agent 模板与模型路由

`init` 命令将 OSS 子代理模板写入 `.codex/agents/`，使用路由后的模型格式：

| 字段 | 值 | 示例 |
|---|---|---|
| `model_provider` | `mimo_adapter`（固定） | `mimo_adapter` |
| `model` | `mimo_adapter/<项目密钥>/<实际模型>` | `mimo_adapter/c8b0cfc9ca15/mimo/deepseek-v4-flash` |

项目密钥是从项目根路径派生的短哈希。适配器服务器解析此格式以提取项目和上游模型，然后将请求路由到正确的 API 密钥和上游 Base URL。

旧的裸格式 `mimo/<model>` 不再支持；请重新运行 `init` 以重新生成模板。

### 多项目使用

你可以初始化多个项目目录——每个目录都有自己的 `.codex-mimo-adapter.env`、带有自己项目密钥的 agent 模板，以及单独的注册表条目。单个 `codex-mimo-adapter run` 实例在启动时加载所有已注册的项目，并按项目密钥路由请求。

要加载新初始化的项目而无需重启适配器，请调用：

```powershell
curl.exe -X POST http://127.0.0.1:4010/admin/refresh -H "Authorization: Bearer $(codex-mimo-adapter auth print-local-token)"
```

`/admin/refresh` 端点会读取注册表并加载尚未在内存中的项目。

### 配置

`init` 将默认运行时设置写入当前项目的 `.codex-mimo-adapter.env`，包括 `CODEX_MIMO_PROJECT_ID`。需要更改存储的 API 密钥、端口、令牌或 SQLite 路径时，编辑该文件。

每个项目目录都有自己的 `.codex-mimo-adapter.env`。适配器还在 `~/.codex-mimo-adapter/project-registry.toml` 维护全局注册表，用于在启动时发现项目。

运行时优先级为 `CLI 参数 > .codex-mimo-adapter.env > 进程环境变量 > 默认值`。
可用变量请参阅下方[环境变量](#环境变量)表。
`run`/`start` 从全局注册表加载所有已注册项目。`check` 从最近的项目 env 文件读取配置。
`auth print-local-token` 从最近的项目 env 或任何已注册项目中查找本地令牌，然后签署适配器级令牌。
项目路由完全由适配器服务器通过 `model = mimo_adapter/<项目密钥>/<实际模型>` 处理。

### 健康检查

```powershell
codex-mimo-adapter check
```

此命令验证本地适配器正在运行（`/health`）且模型端点（`/v1/models`）使用有效令牌响应。

`./scripts/check-local-adapter.ps1` 仍可作为旧版辅助工具使用，但 CLI 命令是主要途径。

需要时运行完整 smoke 测试：

```powershell
./scripts/run-real-smoke.ps1 -ApiKey "<你的密钥>"
```

如需单个文档，请从 [docs/USAGE.zh-CN.md](docs/USAGE.zh-CN.md) 开始。真实验证结果在 [docs/REAL_VALIDATION_2026-06-25.zh-CN.md](docs/REAL_VALIDATION_2026-06-25.zh-CN.md)。

## 环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `MIMO_API_KEY` | 必填 | MiMo Token Plan 的 API 密钥 |
| `CODEX_MIMO_LOCAL_TOKEN` | 由 `init` 生成 | 本地调用者所需的 Bearer 令牌。如果为空，跳过认证 |
| `CODEX_MIMO_PROJECT_ID` | 由 `init` 生成 | 存储在 `.codex-mimo-adapter.env` 中的项目标识符 |
| `CODEX_MIMO_HOST` | `127.0.0.1` | 监听地址 |
| `CODEX_MIMO_PORT` | `4010` | 监听端口 |
| `MIMO_API_BASE_URL` | `https://token-plan-cn.xiaomimimo.com/v1` | MiMo Token Plan 上游 Base URL |
| `CODEX_MIMO_STATE_DB` | `.codex-mimo/state.sqlite` | SQLite 状态数据库路径（相对于项目根目录） |
| `CODEX_MIMO_STATE_TTL_SECONDS` | `21600` | 状态过期时间，6 小时 |
| `CODEX_MIMO_TIMEOUT_SECONDS` | `300` | 上游请求超时 |
| `CODEX_MIMO_MAX_REQUEST_BYTES` | `8388608` | 最大请求体大小，8 MB |
| `CODEX_MIMO_MAX_CONCURRENCY` | `8` | 最大并发上游请求数，启动时读取 |
| `RUST_LOG` | `codex_mimo_adapter=info` | 日志过滤器。使用 `codex_mimo_adapter=debug` 获取详细诊断信息 |

上游 API 密钥和本地客户端令牌必须不同。适配器不会记录任一令牌。

如果看到 `adapter concurrency limit reached`，请先检查当前项目的 `.codex-mimo-adapter.env`。
此消息表示适配器自身的 `CODEX_MIMO_MAX_CONCURRENCY` 限制已耗尽或配置过低；它本身并不能证明上游模型厂商只支持单个请求。

## API 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| `POST` | `/v1/responses` | Responses API，支持流式和非流式 |
| `GET` | `/v1/models` | 列出可用模型。模型 ID 使用 `mimo_adapter/<项目密钥>/mimo/<id>` 前缀。需要适配器 Bearer 令牌 |
| `GET` | `/health` | 健康检查 |
| `POST` | `/admin/refresh` | 热重载注册表中的项目，无需重启适配器。需要适配器 Bearer 令牌 |

## 测试

```bash
cargo test
```

真实上游 smoke 测试：

```bash
MIMO_API_KEY="你的密钥" cargo test --test e2e_real_smoke test_e2e_real_validation_suite -- --ignored --nocapture
```

## 当前状态

适配器可用于与 MiMo Token Plan 的自托管 Codex 子代理路由。文本、流式、工具调用、自定义工具、工具搜索、续传和多模态保护路径已有 mock 覆盖，且已对当前设置运行了真实 MiMo Token Plan smoke 验证。

参见 [docs/ROADMAP.md](docs/ROADMAP.md) 了解状态和后续里程碑。

## 更多文档

| 文件 | 用途 |
|---|---|
| [docs/USAGE.zh-CN.md](docs/USAGE.zh-CN.md) | 简短的自用设置和故障排除指南 |
| [docs/REAL_VALIDATION_2026-06-25.zh-CN.md](docs/REAL_VALIDATION_2026-06-25.zh-CN.md) | 最新真实上游 smoke 和部分 Codex 验证记录 |
| [docs/VALIDATION.zh-CN.md](docs/VALIDATION.zh-CN.md) | 完整的手动验证清单 |
| [docs/DIAGNOSTICS.md](docs/DIAGNOSTICS.md) | 运行时诊断和日志解读 |
| [scripts/install-user-provider.ps1](scripts/install-user-provider.ps1) | 旧版包装器，现指向 `codex-mimo-adapter init` |
| [scripts/check-local-adapter.ps1](scripts/check-local-adapter.ps1) | 旧版 PowerShell 辅助工具，用于 `/health` 和 `/v1/models` |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | 兼容性范围和非目标 |
| [docs/ROADMAP.md](docs/ROADMAP.md) | 当前状态和未来计划 |

## 明确的非目标

不计划实现：

- 完整的 cc-switch 移植
- Provider 聚合平台
- UI、hooks、插件、statusLine 或上游会话管理
- 自动模型回退/路由
- 剥离媒体后的自动多模态重试
- 静默多模态降级（让纯文本模型假装看到了媒体）

参见 [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) 了解完整兼容性范围。
