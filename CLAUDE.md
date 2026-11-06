# agent-office

**语言：** 默认简体中文回复；代码、标识符、代码注释用英文（跟随现有代码）。

置顶的像素办公室窗口，每个 Claude Code 会话是一个小人。功能和配置见 README.md。

## 结构

- `src/`：agent-office 自己的代码。窗口和渲染接线在 `floating/`，任务摘要和卡点检测在 `tasks/`，hooks（`connect` / `hook`）在 `hook.rs`，事件管线在 `pipeline.rs`，跳回会话在 `jump.rs`。
- `vendor/pixtuoid-core`、`vendor/pixtuoid-scene`：pixtuoid 0.21.0（MIT），通过 `Cargo.toml` 的 `[patch.crates-io]` 接入，已经改成本项目的样子，改过的地方都标了 `agent-office`。改 vendor 前先读该 crate 的 `AGENTS.md`，新改动同样要标 `agent-office`。
- `tools/*.py`：美术脚本。读的是 cargo registry 里的上游原版 sprite，写进 `vendor/pixtuoid-scene/sprites/default`。

## 验证

- 改完运行 `cargo test --release`，所有测试都要过。
- 改了 `vendor/pixtuoid-core`（reducer、状态、decoder、sprite 格式）：在该目录运行 `cargo test --release`。依赖 `harness` feature 和上游 `tests/sources` 采集数据的测试与 examples 在这里不编译（发布包没带这些数据）。
- 改了 vendor 的美术或渲染：在 `vendor/pixtuoid-scene` 里运行 `cargo test --release`。画面确实有意改动时，用 `SNAPSHOTS=overwrite cargo test --release` 更新像素金样，并说明改了什么。
- 只有在 macOS 本机才能看画面：`cargo run --release -- --shot out.png` 跑 20 秒后存一张截图。云端（Linux，没有显示器，环境变量 `CLAUDE_CODE_REMOTE=true`）跑不了窗口、`--shot` 和跳转。在云端只做 build 和 test，并在结果里写明画面没有验证。
- 测试要写清楚"这个行为为什么重要"。业务逻辑变了测试还能过，说明测试写错了。

## 约定

- 最小改动：只改该改的，不顺手改相邻的代码、注释或格式。注释只写为什么。
- 不要把内部地址、密钥、真实会话名或个人路径写进仓库。截图要用假会话生成：设 `CLAUDE_CONFIG_DIR` 指向一个只有 demo transcript 的目录。
- 摘要接口只通过 `AGENT_OFFICE_API_BASE` / `AGENT_OFFICE_API_KEY` / `AGENT_OFFICE_MODEL` 配置，代码里不写默认地址。
- 未经要求不新增文档；commit / push 按全局「放行与拍板」规则。
