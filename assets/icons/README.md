# WayX 图标

`wayx.svg` 是用户提供的 X11 标志原文件。图标保留黑色图形与透明背景。

- `wayx-*.png`：16–1024 像素的 PNG。
- `wayx.ico`：Windows 多尺寸图标，嵌入 `wayx.exe`。
- `wayx.icns`：macOS 图标文件，启动时由 AppKit 设置为 Dock 图标；当前没有 `.app` 包，Finder 的应用包图标另需打包支持。
- `wayx-128.rgba`：编译进程序的窗口图标，避免增加运行时图片解码依赖。

安装 CairoSVG、Pillow 后，在仓库根目录执行 `python3 scripts/generate-icons.py` 重新生成。
运行包在 `share/icons` 中附带 SVG、256px PNG、ICO 和 ICNS。
