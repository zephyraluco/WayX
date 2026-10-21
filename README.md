# WayX

按 **XLibre X Server Core + C DDX Bridge + Rust + winit + wgpu + russh** 架构实现的 X11 转发客户端。

当前版本打通 Linux 上的 **rootful MVP**：远端 X11 应用通过 SSH 标准 X11 forwarding 接入本地 XLibre；XLibre FB/Pixman 生成的软件帧缓冲经 Damage 上传到 Rust，最终由 wgpu 呈现在 winit 原生窗口中。

**不是完整的跨平台 X Server 发行版。** Rust 显示后端面向 Win32 / AppKit / Linux Wayland、X11；XLibre DDX 当前只在 Linux 构建和运行验证。逐 X11 顶层窗口的 rootless 映射、完整 IME/XIM 和 Windows/macOS 的 XLibre 集成尚未完成。Linux 使用 Wayland 作为本地窗口系统，不等于支持远端原生 Wayland 协议转发。

## 工程结构

| 路径 | 作用 |
| --- | --- |
| `src/backend.rs` | 窗口生命周期、父子关系、焦点、帧缓冲、Damage、输入队列、剪贴板状态 |
| `src/bridge.rs`, `include/wayx.h` | 版本化 C ABI、回调表、缓冲区校验、线程同步 |
| `ddx/` | XLibre 软件 framebuffer DDX、Damage、键鼠输入接入 |
| `src/desktop.rs` | winit 原生窗口/事件、wgpu 纹理上传与呈现、本地剪贴板 |
| `src/selection.rs` | X11 CLIPBOARD 的 UTF8_STRING/STRING/TARGETS/TIMESTAMP/INCR 转换 |
| `src/ssh.rs` | russh 会话、主机密钥校验、X11 channel、认证 cookie 替换 |
| `src/auth.rs` | 本地 Xauthority 文件读取 |
| `tests/` | C ABI、真实 XLibre 绘制/输入/剪贴板、加密 SSH 通道测试 |

## 构建

需要 Rust stable（当前使用 1.99.1）、C 编译器、pkg-config；构建 XLibre 还需要 Python、Meson、Ninja、Git 和 X11 开发库。

Debian 13：

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config git python3 meson ninja-build \
  libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev wayland-protocols \
  libpixman-1-dev libxkbfile-dev libxfont-dev libmd-dev xtrans-dev x11proto-dev \
  xkb-data x11-xkb-utils xauth libvulkan1 mesa-vulkan-drivers \
  libxcursor1 libx11-xcb1 libxi6 libxkbcommon-x11-0
cargo build --locked
```

本次云环境已准备了独立工具目录，在每个 shell 中先执行：

```sh
. /workspace/.wayx-tools/activate.sh
cd /workspace/WayX
```

只测试 Rust 状态管理和 C Bridge、不使用 GPU、SSH 或窗口系统：

```sh
cargo test --locked --no-default-features --lib
```

## 显示后端演示

在有桌面会话的主机运行：

```sh
cargo run --locked -- demo
# 自动化：成功呈现三帧后退出
cargo run --locked -- demo --frames 3
```

没有桌面时，可用真实 GPU 或 Mesa 软件 Vulkan 验证纹理、着色器和回读：

```sh
cargo run --locked -- gpu-smoke
```

此演示不实现 X11 协议；要运行 X11 程序，需要下述真实 XLibre 核心。

## 构建并运行 XLibre DDX（Linux）

```sh
bash scripts/build-xlibre.sh
```

云环境编译 C 核心前还需启用 Meson 与本地 C 库路径：

```sh
. /workspace/.wayx-tools/xlibre-env.sh
```

脚本拉取固定提交 `a8192aa70998a803155bdc2e73b7564a1415f33a`，在 `build/` 中生成 DDX 插件，不修改任何上游原始 DDX 文件。可用 `WAYX_XLIBRE_SOURCE` 指向已有的同一提交源码；该目录应专用于构建，脚本会添加 `hw/wayx` 和 Meson 注册。`WAYX_BUILD_JOBS` 默认 3。

先生成仅用于本地 X Server 的 Xauthority 文件（依赖 `xauth`）：

```sh
mkdir -p .cache
chmod 700 .cache
python3 scripts/create-xauthority.py --display 100 --output .cache/Xauthority
cargo run --locked -- serve --core build/xlibre/hw/wayx/libwayx-xserver.so -- \
  :100 -screen 0 1280x800x24 -crtcs 1@1280x800 -auth .cache/Xauthority -nolisten tcp
```

原生窗口必须在主线程运行；XLibre 在专用工作线程运行。关闭原生桌面窗口会请求 XLibre 正常退出。此版本每进程只运行一个 XLibre 核心。

本机另一个终端可以运行 X11 客户端：

```sh
DISPLAY=:100 XAUTHORITY=$PWD/.cache/Xauthority xterm
```

单个原生窗口承载整个 X 屏幕；内部 X 窗口由 XLibre 管理。需要桌面装饰时，可在该显示中启动自己选择的 X11 窗口管理器。拖动原生窗口尺寸当前只缩放画面，不改变 X 屏幕分辨率。

## SSH X11 forwarding

保持本地 `serve` 运行。远端 sshd 须启用 `X11Forwarding yes` 并安装 `xauth`。本地主机的 `known_hosts` 必须已经通过可信途径记录远端主机公钥。

```sh
cargo run --locked -- ssh \
  --host example.org --port 22 --user alice \
  --identity /path/to/id_ed25519 --known-hosts /path/to/known_hosts \
  --xauthority .cache/Xauthority --display 100 --command 'xterm'
```

Linux/macOS 优先连接本地 Unix X11 socket；没有 Unix socket时才尝试回环 TCP，Windows 使用回环 TCP。无需为 Linux 示例开放 X11 TCP 监听。

本地真实 cookie 不交给远端：为 SSH 会话生成独立随机 cookie，验证每条 X11 连接的 setup 请求后，再替换成本地 cookie。错误 cookie、未知/变更的主机密钥会被拒绝。当前不支持主机证书 CA 信任策略。支持 Ed25519、ECDSA、RSA 身份密钥；加密私钥可通过 `--key-passphrase-env VARIABLE_NAME` 指定密码所在的环境变量，避免把密码放进命令参数。

`--command` 是远端 shell 命令；退出码传递给本地调用方。每会话最多 64 条并发 X11 连接，认证 setup 有 10 秒超时。

## 验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
bash scripts/test-c-bridge.sh
WAYX_TEST_CORE=$PWD/build/xlibre/hw/wayx/libwayx-xserver.so \
  cargo test --locked --test xlibre -- --ignored --nocapture
cargo run --locked -- gpu-smoke
```

默认测试包括本地真实加密 SSH 会话，不需要任何远端凭据。XLibre 集成测试必须显式运行；测试会建立隔离的 `:191` Unix 显示、禁用 TCP，并在测试结束时关闭服务。它验证真实 X11 绘制、键盘/鼠标回传、中文剪贴板双向转换和大文本 INCR 双向传输；不以打开端口或零测试运行作为成功依据。

三平台 CI 已配置，但工作流必须在 GitHub 上运行后才能确认原生结果。当前实现边界与线程模型见 [架构说明](docs/architecture.md)。

## 许可

本仓库自有代码使用 MIT。XLibre、Pixman 及其他依赖保留各自许可；XLibre 按文件包含不同许可，分发其源码或二进制时须单独遵守。构建脚本从上游获取代码，不将其声明为本仓库自有代码。
