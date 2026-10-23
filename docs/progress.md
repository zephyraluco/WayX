# 功能实施与验证进度

按用户指定顺序推进。每个大功能完成后独立提交；没有对应运行证据的能力不标为完成。

| 顺序 | 功能 | 状态 | 验证要求 |
| --- | --- | --- | --- |
| 1 | Rootless 逐窗口映射 | 已实现，Linux 验证通过 | Composite 独立 Pixmap、完全遮挡下绘制、嵌套子窗口、输入路由、map/unmap/resize/destroy；实际两个原生窗口与独立关闭 |
| 2 | 窗口管理 | Linux X11 已实现并验证；其他宿主平台待验收 | 双向移动/resize、焦点与层级、WM_DELETE_WINDOW、ICCCM 大小约束、瞬态/弹出窗口 |
| 3 | Windows/macOS XLibre 核心 | 已实现，Windows/macOS 原生 CI 通过 | 两个平台核心的真实编译、链接、打包和 GUI/协议验证；不以 Rust 静态检查代替 |
| 4 | IME/XIM | 已实现，Linux 协议验证通过；宿主输入法待交互验收 | Unicode 提交、焦点生命周期、回调预编辑及本地覆盖层 |
| 5 | 平滑滚动 | 已实现，Linux 协议验证通过；跨平台交互待验收 | XI2 亚单位滚动、双轴方向、兼容按钮与输入顺序 |
| 6 | 完整剪贴板 | 待实现 | 图片、文件列表、PRIMARY、持久化及已有文本/INCR 回归 |
| 7 | SSH 使用体验 | 部分实现 | 密码/私钥认证已实现；Agent、OpenSSH 配置、ProxyJump、重连和连接管理待完成 |
| 8 | 显示扩展 | 部分实现，Linux 验证通过 | Rootless 自动宿主布局、负坐标、原点变更、多输出、准确指针与拖动零 RandR；Windows/macOS 新增路径待原生 CI 验证，其他像素格式待实现 |
| 9 | 发行打包 | 待实现 | 各平台安装包、依赖、签名和更新验证 |

硬件图形加速（宿主 GPU GLX、DRI2/DRI3）及服务端间接 OpenGL 不纳入后续规划。

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


## 第三项验证

提交 `a496bce` 的 [原生核心 CI](https://github.com/zephyraluco/WayX/actions/runs/39709583993)
与 [常规三平台 CI](https://github.com/zephyraluco/WayX/actions/runs/39709583984) 均通过。

- Windows 与 macOS 均实际编译、链接核心，并执行 `xlibre`、`rootless`、
  `window_manager` 三项真实 X11 集成测试：绘制、输入、文本/INCR、重定向
  Pixmap、窗口请求、状态、焦点和关闭协议。
- Windows 实际创建 Win32 窗口并以 Microsoft Basic Render Driver / DX12
  呈现；macOS 实际创建 AppKit 窗口并使用 Metal 呈现。
- 两个平台均验证搬移后的 release 运行包；Windows 还隐藏原始依赖目录，
  并从含空格的目录启动，确认 DLL、XKB 编译器及数据随包提供。
- Windows 使用固定版本 vcpkg C 依赖、原生 UCRT 工具链和校验 SHA-256 的
  协议头、libxkbfile、xkbcomp 与 XKB 数据源包。补丁覆盖 LLP64 指针宽度、
  服务端头文件隔离、函数声明及 cmd.exe 引号处理；固定源码替换和重复应用已检查。
- Meson 实际产物路径记录在 `build/xlibre/wayx-core-path.txt`，不假定动态库后缀。
  `WAYX_XKB_DIR` / `WAYX_XKB_BIN_DIR` 支持搬移后的 XKB 数据与工具定位。

此项不代表第二项在 Windows/macOS 的逐窗口原生交互均已验收，也不代表第九项
正式安装器、可信签名与更新已经完成。IME/XIM 的后续实现状态见上表和架构文档。

## 验证中发现的回归

- SSH 在数据及退出码已完整验证后的服务端收尾，可能返回 ConnectionReset
  或 Windows ConnectionAborted（10053）；仅在这一阶段接受这些结果，其他错误
  仍失败。连续十轮加密会话与三平台 CI 通过，不计为第七项 SSH 使用体验完成。
- release 原生窗口验证发现卸载核心后信号处理器悬空，调试栈定位到 dlclose。
  现将核心库保持加载至进程退出，并按平台能力禁用全局调度定时器。
  Linux release rootful/rootless 渲染、两个独立窗口与正常退出连续三次通过。
- 本地 fmt、Clippy、全特性测试、三项显式真实核心测试及 C ABI 检查通过。
  Linux CI 使用最终 release 二进制执行原生窗口检查。

## 下载产物

每次推送为三个平台独立构建 release 完整包与测试附件，然后由本平台的新 runner
下载测试，不等待其他平台。构建包上传在测试之前，测试失败仍保留可下载产物 7 天；
构建成功与验证通过分别显示，不把可下载包等同于已通过测试。

运行包名称：`<平台>-core-runtime-<x64/arm64>-<完整SHA>`；测试附件和诊断独立上传。
Linux 也提供包含客户端、核心、XKB 和非系统动态依赖的完整包。manifest 使用实际
ABI v5、Rust target、源 SHA 和文件哈希。测试不重新编译产品或夹具。

## SSH 密码认证增量

- 新增 `--password` 和无私钥时默认密码登录；在主机密钥验证后从终端无回显读取。
  保留私钥及其独立的解密口令配置，不接受命令行明文密码。暂不支持
  keyboard-interactive/PAM 多轮交互。
- 请求确认改为持续读取到 Success/Failure，等待期间转发 stdout/stderr 并处理
  通道关闭/退出，不将窗口调整或提前输出误判为服务器拒绝。
- 加密回环测试覆盖密码与私钥转发、cookie 替换、错误密码拒绝和命令明确拒绝；
  真实用户服务器的密码策略与应用环境仍以现场连接结果为准。

## Rootless 宿主桌面映射验证

- `desktop_layout` 真实协议测试：屏幕右下点击、负坐标、2→3→1 输出、原点变更保持宿主位置、Rust root 4 字节、300 步拖动零 RandR 广播。
- Linux 原生自动布局下 rootful/rootless、GLX、静止零额外 present、空载 CPU 和带重绘的拖动回归通过；冲突的手工屏幕参数明确失败。
- Windows 使用独立隐藏顶层窗口接收显示变化，X11 使用宿主 RandR readiness，其他后端保留布局兼容检查。Windows/macOS 的真实热插拔与不同 DPI 体验仍需原生验收。
- ABI v5；客户端和 XLibre 核心必须一起更新。rootless 不再根据窗口拖动扩屏，C root 存储仍在宿主布局变化时正常调整。

## 共享 GPU 生命周期验证

- App 懒初始化并持有一套 Instance/Adapter/Device/Queue；窗口保留独立 Surface 与图像，管线按格式缓存，最后一个窗口关闭后仍复用设备。
- Linux Vulkan 软件驱动：双窗口独立关闭、resize、200×200/1600×1000 各预热四轮再测十二轮，设备计数保持一套；回收完成后的 Surface/纹理/绑定组归零，大窗口测量阶段私有驻留增长约 0.1 MiB。
- GPU smoke、原生 rootful/rootless、GLX、CPU 和持续重绘拖动回归通过。Linux/Windows CI 增加生命周期资源及内存增长预算；Windows 私有提交量与实际 Intel 驱动仍待原生 CI/实机确认。
- 此项不改变 C ABI v5，也不要求关闭全部窗口后回到 GPU 初始化前的内存基线。

## 全局重构

- `97016d6`：XID 原生窗口索引、元数据快照和属性差量应用；Surface 获取/呈现、
  窗口创建与原生剪贴板调用移出 Backend 锁。
- `5a17d3d`：DDX 按窗口分类脏队列、缓存约束、独立生命周期扫描；真实 ABI 测试
  断言移动、标题和无关属性的输出范围。
- `75c9438`：输出提交不可变元数据、XID incarnation、连续像素版本；ResizeSync
  统一持有尺寸目标发出状态；拖动不重复设置原生约束。
- `c0e4b5b`：有界稀疏 Damage、shader 不透明 alpha、IME 旧覆盖区域局部恢复。
- 模块整理：渲染器、X11 会话分发、Selection 服务和 readiness 分离；汇总有实际
  任务的维护截止时间，无任务时等待通知。C ABI 保持 v5。

本轮本地验收使用 Linux X11 与 Mesa 软件 Vulkan；Windows/macOS 交由相同提交的
原生 CI 验证，实际 Windows 驱动内存与宿主输入法仍需现场验收。

## CI/CD 工作流整理

- `test.yml`：checks 执行 Format / Clippy；三个平台测试 job 仅运行 Rust 单元测试。
- `integration.yml`：三个独立平台 job 执行 Build → Bundle → Test → Upload，
  从归档解压完整包后测试；夹具只在 runner 本地复用，不上传。
- 仅上传测试通过的 runtime 和始终尝试保留的 result。
- `release.yml`：发布版本标签对应、默认分支同一提交已通过验证的三平台运行包，
  校验归档和 manifest 后复用产物，不重新编译。
- 保留 SSH、C ABI、协议、GLX/OGRE、XIM、原生 GUI、CPU、几何和 GPU 生命周期覆盖。
