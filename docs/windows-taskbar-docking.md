# Windows 任务栏吸附

Windows 上把紧凑播放条拖到水平任务栏附近（上下 40 DIP 内）会出现描边提示，松手后播放条沿
`cubic-bezier(.77,0,.175,1)` 240ms 飞进任务栏，变成一条 380 DIP 宽的歌词条；点条上的返回
按钮反向飞回原位。系统开启减少动态效果时只做 120ms 原地淡入淡出。

## 结构

- **播放器窗口不动**：`main` 永远是顶层窗口，吸附时只是隐藏，父窗口、样式、尺寸都不变，
  播放会话也只有这一个。
- **歌词条是独立窗口**：`taskbar-lyrics` 是透明、无边框、不可聚焦的 WebView，加载
  `taskbar-overlay.html` 这个轻量入口（不挂 App、Pinia、Player）。它只渲染主窗口发来的
  展示快照，按钮动作原样转回主窗口执行。
- **原生 helper**：`scripts/windows-taskbar-native.cs` 由 `build.rs` 用系统自带的 .NET
  Framework 编译器编译并随包分发。Rust 通过 stdin/stdout 的 JSON 行协议让它探测任务栏
  （空白区、DPI、前台是否全屏）并把歌词条 `SetParent` 进任务栏；helper 收到 EOF 或
  `detach` 时自行还原。它不向 Explorer 注入任何代码。
- **控制器**：`src-tauri/src/taskbar_docking/` 每 500ms 收敛一次窗口状态；纯几何与载荷
  校验在 `taskbar_docking_model.rs`，在所有平台的 CI 上测试。

## 不变量

- 前台是全屏程序或探测结果不可信时，两个窗口都隐藏，吸附意图保留，恢复后不抢焦点。
- 歌词条只在画完当前飞行阶段（按 token 确认）后才显示，绝不闪出旧内容。
- 吸附状态下无法撤销的附着（helper 意外退出）直接销毁歌词条窗口，由控制器重建一个干净的。
- 窗口回调运行在 UI 线程，只读写原子量；持有状态锁的工作线程可能正在等 UI 线程。
- 关闭歌词条只隐藏可见层，播放继续；托盘或第二实例重新打开时恢复。
