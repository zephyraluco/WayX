# WayX

WayX 是基于 **XLibre + Rust + winit + wgpu** 的本地 X Server。
远端应用通过 OpenSSH、PuTTY 等外部工具的 X11 forwarding 连接本地 XLibre，
软件帧缓冲由 wgpu 呈现在原生窗口中。WayX 不内置 SSH 客户端。
宿主支持 Windows、macOS 和 Linux（X11/Wayland）；macOS 不需要 XQuartz。

## 功能与限制

- **Rootful**：在一个原生窗口中显示 X 桌面，单输出桌面支持窗口缩放与 RandR 分辨率同步。
- **Rootless**：每个 X11 顶层窗口独立显示，支持窗口管理、标题、焦点、尺寸约束和关闭；使用宿主多屏布局，最多 16 个输出。
- **输入**：键鼠、XI2 平滑滚动、原生 IME 到 XIM 的文本提交与预编辑。
- **剪贴板**：双向文本同步，支持 UTF-8 和 INCR 大文本传输。
- **外部 SSH**：兼容标准 X11 forwarding；登录、主机密钥、Agent 和转发 cookie 由 SSH 工具管理。
- **OpenGL**：GLX 1.4 协商，应用使用远端 Mesa 软件渲染，图像通过 X11 传输。

当前为实验性运行包。显示格式为 32-bpp XRGB，不支持透明 ARGB、远端原生 Wayland
协议、服务端间接 OpenGL 或 DRI2/DRI3。Rootless 下不要另外启动 X11 窗口管理器；
Wayland/macOS 的窗口定位和瞬态父窗口关系受宿主接口限制。物理输入法、触控板、
不同 DPI 与热插拔体验仍需对应平台的实机验证。

## 下载运行包

从 [Integration 构建产物](https://github.com/zephyraluco/WayX/actions/workflows/integration.yml)
下载与宿主平台和架构匹配的 `wayx-<平台>-<x64或arm64>-<提交SHA>`。
ZIP 解压后顶层即为 `bin`、`share`、`licenses`、`manifest.json` 和启动脚本；
Linux/macOS 另有 `lib` 目录。

Linux 运行包要求 Debian 13 / glibc 2.41 或更新版本。显示服务、GPU 驱动和字体由宿主提供。
macOS 运行包使用 ad-hoc 签名，没有 Developer ID 签名或 Apple 公证；系统拦截时可在
“系统设置 → 隐私与安全性”中允许打开。

### 启动本地显示服务

进入解压目录启动服务。未指定 `-auth` 时默认免认证，也可显式传入 `-ac`。
TCP 监听限制在 `127.0.0.1` / `::1`，不对局域网开放；本机其他进程仍可访问免认证显示。

Linux/macOS：

```sh
chmod +x wayx.sh bin/*
./wayx.sh --rootless -- :100 -nolisten tcp
```

Windows PowerShell：

```powershell
.\wayx.cmd --rootless -- :100 -listen tcp -nolisten local
```

需要本地认证时，创建 Xauthority 后添加 `-auth <文件>`，并配置外部 SSH 客户端使用
相同的认证信息。源码工具 `scripts/create-xauthority.py` 可创建认证文件。`-auth` 与 `-ac` 不能同时使用；认证文件错误不会降级为免认证。

Rootless 模式在应用打开窗口后才显示原生窗口。要使用单窗口桌面，去掉 `--rootless`，
并在 `-- :100` 后添加 `-screen 0 1280x800x24 -crtcs 1@1280x800`。

### 连接远端应用

远端 sshd 需启用 `X11Forwarding yes` 并安装 `xauth`。WayX 不生成或校验 SSH
转发 cookie；这些工作由外部 SSH 客户端和 sshd 完成。

Linux/macOS，在另一终端指定本地显示：

```sh
export DISPLAY=:100
ssh -X alice@example.org xterm
```

只有需要可信 X11 转发并信任远端应用时使用 `ssh -Y`。

Windows 推荐 PuTTY：在 **Connection → SSH → X11** 中启用 X11 forwarding，
将 **X display location** 设为 `localhost:100`。本地免认证时无需配置 Xauthority 文件。
保存会话后，也可通过 Plink 运行：

```powershell
plink -load WayX -X alice@example.org xterm
```

`WayX` 是保存的 PuTTY 会话名。使用支持 X11 转发的 SSH 工具；不要假定所有 Windows
自带 OpenSSH 版本均提供此功能。密码、私钥、Agent、ProxyJump、压缩和主机密钥确认
使用所选 SSH 工具的配置。WayX 与 SSH 独立运行，关闭 SSH 不会自动关闭本地显示服务。

### 输入法与 OpenGL

本地显示自动注册 `@im=wayx`。远端应用需有 UTF-8 locale，
GTK/Qt 应安装相应 XIM 模块，并按工具包需要设置：

```sh
env XMODIFIERS=@im=wayx GTK_IM_MODULE=xim QT_IM_MODULE=xim your-app
```

本地输入法和中日韩字体由宿主提供，单次文本提交上限为 8 KiB。

OpenGL 应用需在远端安装 Mesa 软件驱动，取消 `LIBGL_ALWAYS_INDIRECT`：

```sh
env -u LIBGL_ALWAYS_INDIRECT LIBGL_ALWAYS_SOFTWARE=1 \
  GALLIUM_DRIVER=llvmpipe __GLX_VENDOR_LIBRARY_NAME=mesa glxinfo -B
```

通过外部 SSH 执行上述远端命令即可。运行 RViz 等 Qt 应用时可额外设置
`QT_X11_NO_MITSHM=1`。渲染消耗远端 CPU，动态图像会占用传输带宽。

## 从源码构建

需要 Rust stable、C 编译器、pkg-config、Python 3、Meson、Ninja、Git 及原生 X11 依赖。

Debian 13：

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config git python3 meson ninja-build \
  libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev wayland-protocols \
  libpixman-1-dev libxkbfile-dev libxfont-dev libmd-dev xtrans-dev x11proto-dev \
  xkb-data x11-xkb-utils xauth libvulkan1 mesa-vulkan-drivers \
  libxcursor1 libx11-xcb1 libxi6 libxkbcommon-x11-0
```

macOS：

```sh
brew install meson ninja pkgconf pixman libxfont2 libxkbfile xorgproto xtrans xkbcomp xkeyboard-config
```

Linux/macOS 构建客户端和固定版本的 XLibre 核心：

```sh
cargo build --locked
bash scripts/build-xlibre.sh
```

Windows 使用 MSVC 编译 Rust，vcpkg 与 MSYS2 UCRT64 构建原生依赖；具体命令见
[构建工作流](.github/workflows/integration.yml) 和 [Windows 核心构建脚本](scripts/build-windows-core.sh)。

核心路径写入 `build/xlibre/wayx-core-path.txt`；客户端和核心必须使用相同的 C ABI。
`WAYX_BUILD_JOBS` 控制核心构建并发（默认 3），`WAYX_XLIBRE_SOURCE` 可指定同一固定提交的
专用源码目录。XKB 路径可用 `WAYX_XKB_DIR`、`WAYX_XKB_BIN_DIR` 覆盖。

Linux/macOS 从源码启动：

```sh
cargo run --locked -- serve --rootless --core "$(cat build/xlibre/wayx-core-path.txt)" -- \
  :100 -nolisten tcp
```

## 开发与测试

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --no-default-features --lib
```

桌面演示与无窗口 GPU 检查：

```sh
cargo run --locked -- demo --frames 3
cargo run --locked -- gpu-smoke
```

需要 XLibre 或原生窗口的测试默认跳过。外部 OpenSSH 转发使用隔离的本地 sshd 验证。
测试职责、夹具和显式运行方式见 [tests/README.md](tests/README.md)。

CI 分为静态检查/单元测试、三平台运行包集成测试和标签发布。
Release 复用对应提交通过测试的运行包，校验文件后生成
`wayx-<平台>-<x64或arm64>-<版本标签>.zip`（例如 `wayx-windows-x64-v0.1.0.zip`），
不重新编译。

## 项目结构

| 目录 | 内容 |
| --- | --- |
| `src/` | Rust 显示后端、窗口管理、渲染、输入法与剪贴板 |
| `ddx/`、`include/` | XLibre DDX 与版本化 C ABI |
| `tests/` | Rust 集成测试及 C/C++ 协议客户端 |
| `scripts/` | 构建、打包与测试工具 |

线程模型和组件关系见 [架构说明](docs/architecture.md)。

## 许可

自有代码使用 MIT。XLibre、Pixman 及其他依赖保留各自许可，分发时须遵守运行包
`licenses` 中的对应许可。
