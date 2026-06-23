# codex-opencode-adapter TODO / Handoff v7

> 用途：给新窗口、本地 Claude Code 或后续 agent 接手。
> 核心规则：先对齐、再读代码、再执行；不要根据历史记录直接打勾。

## 0. 当前结论

```txt
代码层面已收口。
cargo test 已由用户在本地执行并通过。
P3 真实 OpenCode Go 实测尚未执行。
```

本轮修复前，旧 TODO 文档中存在过度乐观记录：曾声称 `P0-1 ~ P0-4 + P1 全部完成`，但后续审查发现 streaming、request transform、non-stream response、tool history 回链仍有实际缺口。v7 以当前仓库和用户本地测试结果为准。

## 1. 项目目标

做一个面向 Codex subagent 的双向协议适配层：

```txt
Codex Responses API request
  -> codex-opencode-adapter
  -> OpenCode Go Chat Completions-like request

OpenCode Go Chat Completions-like response / stream
  -> codex-opencode-adapter
  -> Codex Responses API response / stream
```

不是 provider 聚合平台，不完整搬 cc-switch，只移植必要协议行为。

## 2. 参考仓库与本地路径

目标仓库：

```txt
https://github.com/HisenWeb/codex-opencode-adapter
D:\AI-Tools\codex-opencode-adapter
```

参考仓库：

```txt
https://github.com/farion1231/cc-switch
D:\AI-Tools\cc-switch
```

重点参考文件：

```txt
D:\AI-Tools\cc-switch\src-tauri\src\proxy\providers\streaming_codex_chat.rs
D:\AI-Tools\cc-switch\src-tauri\src\proxy\providers\transform_codex_chat.rs
```

## 3. 本轮已修复内容

### 3.1 请求方向：Responses -> Chat

已修复：

- `function_output_call_ids()` 支持从 Responses tool output 读取 `call_id`。
- `custom_tool_call` 转 Chat tool call 时使用 `input -> {"input": ...}`。
- `tool_search_call` 转 Chat tool call 时固定 `function.name = "tool_search"`。
- `custom_tool_call_output` / `tool_search_output` 转 tool message 时保留完整 item JSON。
- `ToolContext` 支持从 `tool_search_output.tools` 补工具上下文。
- `stream_options` 改为透传后合并 `include_usage`。

相关文件：

```txt
src/conversion/responses_to_chat.rs
src/conversion/tool_context.rs
```

### 3.2 非流式响应方向：Chat -> Responses

已修复：

- Chat tool call 缺失 name 时 skip，不再生成 fake `tool`。
- `custom_tool_call` 输出 Responses item 使用 `input` 字段。
- `tool_search_call` 输出 Responses item 使用 `execution: "client"` 和 object `arguments`。
- namespace function_call 输出保留 `namespace`。

相关文件：

```txt
src/conversion/chat_to_responses.rs
src/conversion/tool_context.rs
```

### 3.3 流式响应方向：Chat stream -> Responses stream

已修复：

- streaming tool_call 缺失 name 时 skip，不生成 `unknown_tool`。
- `StreamAssembler` 使用完整 `ToolContext`，支持 custom/tool_search/namespace 形态。
- stream 自然结束但已有实质输出且没有 `finish_reason` 时，标记为 `length`，最终 Responses status 为 `incomplete`。
- terminal finalize/fail 幂等。

相关文件：

```txt
src/conversion/stream_chat_to_responses.rs
src/server.rs
```

## 4. 已写入的关键 commits

```txt
a3694a2 fix responses to chat tool history conversion
91b1c7b add minimal tool context lookup helpers
16d5943 fix nonstream tool response shapes
527afc7 fix streaming tool lifecycle edge cases
07c90e6 pass tool context into stream assembler
e4299b2 fix(server, conversion): 修复流式响应的截断和收尾逻辑
373e81a test repaired conversion regressions
```

## 5. 回归测试

新增：

```txt
tests/regression_conversion.rs
```

覆盖：

- `function_call_output` / `custom_tool_call_output` 使用 `call_id` 回链。
- `custom_tool_call` 转 Chat tool_call。
- `tool_search_call` 转 Chat tool_call。
- stream 有输出但无 `finish_reason` 时可 finalize 为 `incomplete`。

用户本地验证：

```txt
cargo test 通过
```

## 6. 当前状态表

| 阶段 | 状态 | 说明 |
|---|---|---|
| P0-1 streaming tool_call 生命周期 | 已修复 | 显式状态、index 绑定、id/name ready 后 start、terminal 幂等 |
| P0-2 基础测试 | 已补关键回归 | 新增 `tests/regression_conversion.rs` |
| P0-3 stream 收口 | 已修复 | 有输出无 finish_reason -> length/incomplete |
| P0-4 function_call_output 回链 | 已修复 | `call_id` 回链已覆盖测试 |
| P1 请求转换 | 已修复关键缺口 | custom/tool_search/output/tools/stream_options |
| P2 非流式响应转换 | 已修复关键缺口 | custom/tool_search/namespace/missing name |
| P3 OpenCode Go 实测 | 未执行 | 需要真实 OpenCode Go API key / Codex 子代理环境 |
| P4 文档与清理 | 部分完成 | 当前 v7 已收敛 TODO；README 可后续再精简 |

## 7. 仍未做 / 不要误判

- 未做真实 OpenCode Go 端到端测试。
- 未验证 Codex App / Codex subagent 实际接入体验。
- 未做长时间 streaming 压测。
- 未做 provider 平台化。
- 未保证所有 Responses API 边缘字段 100% 兼容，只保证本轮识别出的 custom/tool_search/namespace/tool output/stream 收口缺口已修复并测试通过。

## 8. 后续建议

### 下一步 P3：真实接入测试

在本地配置 OpenCode Go API key 后测试：

```txt
1. 纯文本请求
2. 普通 streaming 文本
3. 一次 function_call
4. function_call_output continuation
5. custom_tool_call
6. tool_search_call
7. stream 中途自然结束/异常结束
```

### 若失败，优先排查文件

```txt
src/conversion/responses_to_chat.rs
src/conversion/chat_to_responses.rs
src/conversion/stream_chat_to_responses.rs
src/conversion/tool_context.rs
src/server.rs
src/state.rs
```

## 9. 新窗口启动提示词

```txt
先不要写代码。

请先读取 docs/codex-opencode-adapter-handoff-todolist.md，并完成接手对齐：

1. 复述项目目标和当前状态。
2. 运行并汇报 git status、git log --oneline -8、cargo test。
3. 阅读 src/conversion/*.rs、src/server.rs、src/state.rs。
4. 对照第 6 节状态表说明哪些已验证，哪些仍需真实接入测试。
5. 我确认后，再进入 P3 OpenCode Go 实测或 P4 文档清理。
```
