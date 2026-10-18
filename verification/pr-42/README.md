# 任务栏歌词验证证据

本目录保存 issue #39 作者所要求的托盘右键、切歌两类触发方式的修复前后实测截图，以及 2026-10-18 重跑记录。

## 截图来源

修复前两张图是此前在 v0.11.1 / `8fffc22` 上实际复现时保存的桌面截图，歌词行约 y=31 处可见横线；本轮没有为了拍 before 再安装旧版本。

修复后两张图来自本轮重新构建并启动的独立 PR #42 预期合并结果。构建候选 `4bfb55bd18e3b0130fe671d3a836409172948fd3` 与原 PR 合并树逐文件一致；后来提交 `1501091` 只修改性能采样测试的三个时间参数，应用代码相同。截图是原生 SendInput 操作后的物理桌面像素，拍摄前没有额外调用 RedrawWindow。

| 触发方式 | 修复前（历史复现截图） | 修复后（本轮独立构建） |
| --- | --- | --- |
| 托盘图标右键，关闭菜单 | ![修复前托盘右键](before-tray.png) | ![修复后托盘右键](after-tray.png) |
| 点击下一首 | ![修复前切歌](before-next.png) | ![修复后切歌](after-next.png) |

另附本轮真实打开的托盘菜单：[after-tray-menu.png](after-tray-menu.png)。

## 本轮结果

- 两个 PR 的 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`bun run typecheck` 和 `bun run build:tauri:renderer` 均通过。
- Bun 1.4.3 / Windows：两个独立候选均为 499 通过、2 失败；两项失败均为本机创建符号链接时的 EPERM，名称见 verification.json。
- PR #41 的显式 ignored 前台全屏 fixture 回归通过。没有把该结果记作 APEX 游戏实测。
- PR #42 独立程序：三轮真实托盘右键/关闭后未见横线，真实点击下一首后曲目 ID 变化且未见横线；两轮吸附/点击合计 476 帧，记录的横线和隐藏事件均为 0。
- 播放曲目、暂停状态、原窗口位置已恢复，进度恢复到原值 1 秒内；测试程序通过真实托盘“退出”结束，退出码 0。安装文件没有替换。

可执行文件 SHA-256：`27bece549473299af1c4aba058e6164372c647bde72cc6ca8f8ff7d209f467ca`。

Apple Silicon 此前失败发生在已有性能采样用例的 100ms 真实 ps 采样间隔检查，和 macOS 条件编译无关。本轮将测试改为 500ms 间隔、1 秒测量，仍保留两个真实样本和原有验证约束；当前 CI 的最终结果以 [run 39110706657](https://github.com/nagi-studio/YesPlayMusic/actions/runs/39110706657) 为准。
