# 功能实施与验证进度

按用户指定顺序推进。每个大功能完成后独立提交；没有对应运行证据的能力不标为完成。

| 顺序 | 功能 | 状态 | 验证要求 |
| --- | --- | --- | --- |
| 1 | Rootless 逐窗口映射 | 已实现，Linux 验证通过 | Composite 独立 Pixmap、完全遮挡下绘制、嵌套子窗口、输入路由、map/unmap/resize/destroy；实际两个原生窗口与独立关闭 |
| 2 | 完整窗口管理 | 待实现 | 双向移动/resize、焦点与层级、WM_DELETE_WINDOW、ICCCM 大小约束、瞬态/弹出窗口 |
| 3 | Windows/macOS XLibre 核心 | 待实现 | 两个平台核心的真实编译、链接、打包和 GUI/协议验证；不以 Rust 静态检查代替 |
| 4 | IME/XIM | 待实现 | 中日韩组合输入与文本提交到真实 X11 客户端 |
| 5 | 平滑滚动 | 待实现 | 亚单位滚动累积、方向、横向滚动、输入顺序 |
| 6 | 完整剪贴板 | 待实现 | 图片、文件列表、PRIMARY、持久化及已有文本/INCR 回归 |
| 7 | SSH 使用体验 | 待实现 | Agent、OpenSSH 配置、ProxyJump、重连和连接管理 |
| 8 | 显示扩展 | 待实现 | 多屏、动态分辨率、像素格式与 GLX/DRI 路径 |
| 9 | 发行打包 | 待实现 | 各平台安装包、依赖、签名和更新验证 |

## 第一项验证

- `cargo test --test rootless -- --ignored --nocapture`（设置 `WAYX_TEST_CORE`）：真实 XLibre 重定向 Pixmap，不使用截图或模拟窗口。
- `scripts/test-native.py --core ...`：rootful 回归、两个独立 rootless 原生窗口、关闭隔离，以及服务正常退出。
- 原有 Rust、C ABI、rootful XLibre 和 GPU 检查继续作为回归项。

当前 rootless 关闭尚未协商 WM_DELETE_WINDOW，完整关闭协议在第二项实现。透明 ARGB 窗口和平台特定的装饰行为也不计入第一项已验证范围。
