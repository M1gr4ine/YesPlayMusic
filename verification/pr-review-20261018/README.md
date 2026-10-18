# PR #41 / #42 作者反馈复核

2026-10-18，基于 master f6834be。两个 PR 分别验证，保持一个 issue 一个 PR。

#41 移除全局位置 hook，未知探测沿用上次状态，吸附期间才轮询，恢复托盘显示优先级；原生同 HWND 全屏、仅轮询恢复及轮询启停用例通过。

#42 删除 tray-icon 本地补丁及右键任务栏菜单功能，保留 50 ms 节流、原生无标题栏及整窗重绘。截图来自 #42 独立生产构建，源码不含 #41。四轮原生菜单回归与切歌后的横线评分均为 0。

- after-tray.png：真实托盘菜单关闭后的歌词条截图。
- after-next.png：通过原生控件切换下一首后的歌词条截图。

两个独立候选的 Rust workspace tests、Windows 严格 Clippy、fmt、TypeScript 和 renderer 构建通过。Bun 1.4.3 均为 499 通过、2 项 Windows symlink EPERM 失败，未将它们记为通过。

具体源码树、提交及程序/截图 SHA-256 见 verification.json。测试程序已退出。
