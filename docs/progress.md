# 功能实施与验证进度

按用户指定顺序推进。每个大功能完成后独立提交；没有对应运行证据的能力不标为完成。

| 顺序 | 功能 | 状态 | 验证要求 |
| --- | --- | --- | --- |
| 1 | Rootless 逐窗口映射 | 已实现，Linux 验证通过 | Composite 独立 Pixmap、完全遮挡下绘制、嵌套子窗口、输入路由、map/unmap/resize/destroy；实际两个原生窗口与独立关闭 |
| 2 | 窗口管理 | Linux X11 已实现并验证；其他宿主平台待验收 | 双向移动/resize、焦点与层级、WM_DELETE_WINDOW、ICCCM 大小约束、瞬态/弹出窗口 |
| 3 | Windows/macOS XLibre 核心 | 进行中：macOS 原生验证通过；Windows 原生构建验证中 | 两个平台核心的真实编译、链接、打包和 GUI/协议验证；不以 Rust 静态检查代替 |
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

透明 ARGB 窗口不计入第一项已验证范围。

## 第二项验证

- 新增 `window_manager` 显式集成测试：真实 XLibre MapRequest/ConfigureRequest、第二管理器拒绝、客户端列表、焦点、尺寸增量约束、瞬态与弹出标记、EWMH 状态及 WM_DELETE_WINDOW 协商。
- 原生 X11 测试实际观察宿主窗口：两个独立窗口、双向移动/缩放、WM_TRANSIENT_FOR，以及应用收到关闭通知后自行销毁。
- Rust fmt、Clippy、全特性测试、C ABI v2 布局/回调，以及原有 rootful/rootless 绘制输入和文本/INCR 回归通过。
- 最小化的原生 `0×0` 事件只同步状态，不修改 X11 帧缓冲；恢复尺寸及 IconicState 回归通过。
- C ABI v2 创建窗口先隐藏，插件需重建。Wayland 全局位置控制受协议限制；Wayland/macOS 原生瞬态关系不计为已完成，Windows/macOS 核心仍属第三项。


## 第三项进展（未验收）

- 构建脚本不再假定 `.so` 后缀，从 Meson 实际产物生成 `build/xlibre/wayx-core-path.txt`，支持 `.dylib`/`.dll` 的路径发现。
- XKB 编译器及数据位置从原生 pkg-config 获取，可用 WAYX_XKB_BIN_DIR / WAYX_XKB_DIR 在构建及运行时覆盖；输出到可写临时目录。固定源包的数据生成、安装和真实核心使用该数据的输入回归已通过。
- macOS 原生 XLibre 构建、真实 X11 协议测试、AppKit/Metal 呈现和动态库归档 CI 已通过（run 39611199753、39611787101）。
- Windows 测试连接调整为回环 TCP；仅 Rust desktop 跨目标静态检查通过，不代表 XLibre DLL 可用。MSYS2 包目录缺少原生 xfont2、libxkbfile、xkbcomp、xtrans 等必需依赖；现使用固定版本 vcpkg 提供 C 依赖，并以校验 SHA-256 的源包补齐新版协议头、XKB 编译器和数据。新增 Windows 原生构建/协议/Win32 呈现 CI，必须以实际执行结果验收。
- 第三项没有通过两个平台的核心/GUI 验收，因此尚未开始第四项 IME/XIM。

- 核心运行包补充依赖解析、原生动态库与 XKB 数据收集、许可证、启动脚本；macOS 重写加载路径并 ad-hoc 签名，Windows 收集 PE 依赖。CI 需移动包目录后重新启动；Windows 还隐藏原始依赖目录，避免构建机路径掩盖缺包。运行包不是第九项的正式安装器/可信发行签名。

## 验证中发现的回归

Windows 常规 CI 暴露 SSH 关闭的 TCP reset；已等待 russh 传输任务结束，并对完整加密会话连续执行十轮。新增阶段诊断确认 reset 发生在测试服务端收尾、数据及退出码已验证之后；仅允许这一阶段的 ConnectionReset，其余错误继续失败。既有修复在三平台 CI 通过；后续 Windows 又出现相同收尾阶段的 ConnectionAborted（10053），现将这一类型也限定在完整会话验证之后接受，新的 Windows CI 待完成。此项属于验证修复，不代表第七项 SSH 使用体验已完成。

- CI 下载策略：每次推送均运行三平台客户端及原生核心工作流，不再取消旧提交的
  构建或按文件路径跳过核心构建。release 客户端、Linux 核心以及验证通过的
  macOS/Windows 运行包按提交 SHA 命名，上传后保留 7 天。失败任务不发布产物。
- macOS 搬移后的核心运行包已在 run 39620591975 验证通过。Windows xkbcomp
  编译失败定位为 Windows `ERROR` 常量覆盖诊断宏，已针对固定源包修正；完整
  Windows 验收仍待后续 CI。原生运行包客户端改用 release 构建，macOS 包使用
  tar.gz 保留可执行权限；Windows 使用 MSYS2 action 输出的实际路径寻找依赖。

- release 模式实际窗口验证暴露核心卸载后的信号处理器悬空：调试栈定位到
  `dlclose`。嵌入模式禁用进程级调度定时器，客户端将核心库固定至进程退出；
  release rootful/rootless 原生渲染、两个独立窗口和退出连续三次通过。
- Windows xkbcomp 编译及链接已通过（run 39652177310）；现在进入 XLibre
  核心编译。已为固定版本源码中的指针转换补充 LLP64 `intptr_t` 修正，补丁
  可重复应用并经 Linux 真实核心编译及协议回归，Windows 完整验收仍待 CI。
- 本轮本地 fmt、Clippy、全特性测试、三项显式真实 XLibre 集成测试及 C ABI
  检查通过。Linux CI 的原生窗口检查改为执行最终 release 二进制。
