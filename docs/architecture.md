# 架构与实现边界

```mermaid
flowchart TB
  A[Remote X11 Applications] <--> B[russh · SSH X11 Forwarding]
  B <--> C[XLibre Core · FB / Pixman]
  C <--> D[Custom C DDX · ABI v1]
  D <--> E[Rust Window / Framebuffer / Input State]
  E --> F[wgpu · Damage Texture Upload]
  E <--> G[winit · Native Windows and Events]
  F --> G
  C <--> H[Rust ICCCM Selection Adapter]
  H <--> I[Native Clipboard · arboard]
  G --> J[Win32 / AppKit / Linux Wayland or X11]
```

## 所有权与线程

主线程拥有 winit EventLoop、原生窗口、wgpu surface/device/queue、本地剪贴板，以及非阻塞轮询的 X11 Selection 适配器。XLibre 的 `dix_main` 位于独立工作线程；所有 X Server 数据结构只在该线程访问。C DDX 将帧缓冲借用到 Rust 回调中，Rust 在调用返回前完成有界复制，不持有 C 缓冲指针。

共享状态由 `Arc<Mutex<Backend>>` 管理。ABI 使用不透明 handle 和完整 `WayxApi` 回调表，带版本和结构体尺寸检查。禁止把 Rust 对象、字符串或 Vec 布局当成 C 类型。销毁 handle 必须等待所有调用结束。无效非空指针无法由 ABI 安全验证，调用方仍有义务保证指针有效。

动态库在 XLibre 工作线程返回前一直保持加载。原生窗口关闭或 UI 出错后请求 XLibre 退出、等待工作线程结束，再释放资源；不会在核心还运行时卸载插件。

## 显示路径

当前 C DDX 复用固定版本 XLibre VFB 的软件屏幕初始化和输入设备。X11 请求、资源、扩展、Pixmap 以及 Pixman 绘图都由真正的 XLibre 核心实现。屏幕限定为单个 depth-24、32-bpp、little-endian XRGB8888 framebuffer。

Damage 记录在屏幕 Pixmap 上。8 毫秒定时器将受损矩形和实际 stride 传给 Rust。Rust 校验矩形、行跨度与内存范围，将 XRGB alpha 归一化为 255，并合并待呈现的 Damage。wgpu 只上传受损纹理区域，通过全屏三角形和纹理采样呈现；窗口大小变化目前缩放已有帧缓冲。

内存上限：单帧缓冲 256 MiB，所有窗口总计 512 MiB、最多 1024 个窗口，单轴不超过 16384。显卡自身能力可能更低。Rust 的窗口模型维护 ID、父子关系、标题、可见性、焦点和递归销毁，但当前 XLibre DDX 只向模型注册整个 X 屏幕。逐个 X11 顶层窗口的 rootless 桥接未实现；不能把单独调用 `window_create` 当成已完成 rootless 模式。

## 输入

winit 物理键转换成显式 USB HID usage，再由 C DDX 映射为 evdev XKB keycode。没有依赖 Rust enum 序号或直接转型为平台扫描码。鼠标坐标按 framebuffer/原生窗口比例转换；C DDX 调用 XLibre 的 QueuePointerEvents/QueueKeyboardEvents。

按键和鼠标按钮保持有序；只合并相邻、同窗口的 motion。队列有 4096 条上限，溢出明确报错，不静默丢掉 key-up。失焦释放已按下的键和按钮。物理键自动重复交给 XKB。滚轮当前映射为 X11 按钮 4–7，尚未累积亚单位触控板滚动。

IME Commit 已以有界 UTF-8 TEXT 事件传入队列，但 DDX **尚未实现 XIM/XIC 注入**，不会把文本伪装成物理键；IME 不应被当作可用功能。当前 XKB 默认布局为 us；可通过 XLibre 支持的 XKB 配置或 X11 客户端切换布局。

## 剪贴板

C ABI 提供 UTF-8 缓存读写；真实 X11 Selection 协议由 Rust 内部 X11 客户端连接 XLibre 完成。这样不从 DDX 工作线程直接调用原生剪贴板 API。

适配器通过 XFixes 观察 CLIPBOARD 所有者变化，支持 TARGETS、UTF8_STRING、STRING、TIMESTAMP，以及大文本 INCR 接收/发送。单次文本上限 16 MiB，最多 32 个并发 INCR 发送，超时发送会被清理。只同步 CLIPBOARD，不同步 PRIMARY，不实现图片、文件列表、MULTIPLE 或剪贴板管理器持久化。

本地剪贴板由 arboard 使用平台机制处理。Linux Wayland 使用 data-control 支持，实际可用性取决于 compositor/会话策略。X11 选择转换已进行真实 XLibre 测试；macOS/Windows 原生剪贴板仍需对应系统运行 CI 和交互测试。

## SSH 与认证

SSH 会话和所有 X11 channel 使用 russh。只允许已记录的原始主机公钥；未知和变更密钥拒绝连接，不自动信任首次连接，也不绕过证书策略。每个会话产生新的随机 spoof cookie；每条 X11 channel 在连接本地 X Server 前验证 X11 setup 的字节序、协议版本、认证名称、长度和 cookie，再替换成本地 Xauthority 中的 cookie。

本地真实 cookie 不经 SSH X11 forwarding 请求传给远端。Unix 系统优先本地 Unix socket，其他情况使用回环 TCP。默认示例不开放 X11 TCP；不使用 `xhost +` 或 `-ac` 作为用户启动方式。只有隔离的、禁用 TCP 的测试显示使用 `-ac`。

测试以本地临时密钥和真实加密 russh 会话执行，不需要外部服务器、用户密钥或虚构的远端认证。CLI 可通过命名的环境变量读取加密身份密钥的密码。未实现 SSH Agent、OpenSSH config、ProxyJump、连接 UI 或重连恢复。

## 平台状态

| 范围 | Linux | Windows | macOS |
| --- | --- | --- | --- |
| Rust 状态与 C ABI | 运行测试通过 | 已配置原生 CI，待运行 | 已配置原生 CI，待运行 |
| Rust winit/wgpu 显示后端 | 原生 X11 surface 与 Vulkan 软件渲染已运行 | GNU 目标静态检查通过；未链接或运行 | Apple ARM64 目标静态检查通过；未链接或运行 |
| XLibre 自定义 DDX | 固定提交构建、协议与输入测试通过 | 尚未移植/打包 | 尚未移植/打包 |
| 本地 Wayland surface | 已启用编译，当前环境无 compositor，未运行 | 不适用 | 不适用 |
| 远端原生 Wayland 应用 | 不属于 SSH X11 forwarding 支持范围 | 同左 | 同左 |

CI 配置并不等于已执行成功。完整跨平台发行需要分别解决 XLibre 的平台依赖、库打包、线程/信号行为和原生交互验证。此仓库交付的是可验证的 Linux rootful 实现与跨平台 Rust 显示层，不宣称这些后续工作已经完成。
