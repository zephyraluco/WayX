# WayX

WayX 是基于 **XLibre + Rust + winit + wgpu + russh** 的 X11 转发客户端。
远端应用通过 SSH X11 forwarding 连接本地 XLibre，软件帧缓冲由 wgpu 呈现在原生窗口中。
宿主支持 Windows、macOS 和 Linux（X11/Wayland）；macOS 不需要 XQuartz。

## 功能与限制

- **Rootful**：在一个原生窗口中显示 X 桌面，单输出桌面支持窗口缩放与 RandR 分辨率同步。
- **Rootless**：每个 X11 顶层窗口独立显示，支持窗口管理、标题、焦点、尺寸约束和关闭；使用宿主多屏布局，最多 16 个输出。
- **输入**：键鼠、XI2 平滑滚动、原生 IME 到 XIM 的文本提交与预编辑。
- **剪贴板**：双向文本同步，支持 UTF-8 和 INCR 大文本传输。
- **SSH**：密码或私钥认证、主机密钥校验、X11 cookie 替换及压缩协商。
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

进入解压目录，先为显示 `:100` 创建认证文件（需要 Python 3，仅执行一次）：

```sh
python3 -c "import secrets; open('Xauthority', 'xb').write(bytes.fromhex('ffff0000000331303000124d49542d4d414749432d434f4f4b49452d310010') + secrets.token_bytes(16))"
```

Windows 可将 `python3` 替换为 `py -3`。认证文件含本地访问凭据，请限制访问；
服务运行期间不要重新生成。

Linux/macOS：

```sh
chmod 600 Xauthority
chmod +x wayx.sh bin/*
./wayx.sh --rootless -- :100 -nolisten tcp -auth "$PWD/Xauthority"
```

Windows PowerShell：

```powershell
.\wayx.cmd --rootless -- :100 -listen tcp -nolisten local -auth .\Xauthority
```

Rootless 模式在应用打开窗口后才显示原生窗口。要使用单窗口桌面，去掉 `--rootless`，
并在 `-- :100` 后添加 `-screen 0 1280x800x24 -crtcs 1@1280x800`。

### 连接远端应用

远端 sshd 需启用 `X11Forwarding yes` 并安装 `xauth`。主机公钥须通过可信途径核实并
保存在本地 `known_hosts` 中。保持显示服务运行，在另一终端进入同一解压目录后执行：

```sh
./bin/wayx ssh --host example.org --user alice --password \
  --known-hosts "$HOME/.ssh/known_hosts" --xauthority Xauthority \
  --display 100 --command 'xterm'
```

Windows 使用 `.\bin\wayx.exe`，并将路径改为 PowerShell 对应形式。
密码由终端交互输入；私钥认证使用 `--identity /path/to/id_ed25519` 替代 `--password`。
加密私钥可通过 `--key-passphrase-env VARIABLE_NAME` 指定解密密码所在的环境变量。
不支持 keyboard-interactive/PAM 多轮认证或主机证书 CA 信任。

默认协商 `zlib@openssh.com`、`zlib`、`none`；`--compression` 可强制指定其中一种，
指定后不自动回退。远端命令的退出码传递给本地调用方。

### 输入法与 OpenGL

使用 `-auth` 启动时，本地显示自动注册 `@im=wayx`。远端应用需有 UTF-8 locale，
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

将上述命令作为 SSH 的 `--command` 即可。运行 RViz 等 Qt 应用时可额外设置
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
mkdir -p .cache
chmod 700 .cache
# 仅首次创建；已有认证文件时跳过此命令。
python3 scripts/create-xauthority.py --display 100 --output .cache/Xauthority
cargo run --locked -- serve --rootless --core "$(cat build/xlibre/wayx-core-path.txt)" -- \
  :100 -auth .cache/Xauthority -nolisten tcp
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

默认测试包含本地加密 SSH 测试，不需要远端凭据；需要 XLibre 或原生窗口的测试默认跳过。
测试职责、夹具和显式运行方式见 [tests/README.md](tests/README.md)。

CI 分为静态检查/单元测试、三平台运行包集成测试和标签发布。
Release 复用对应提交通过测试的运行包，校验文件后生成
`wayx-<平台>-<x64或arm64>-<版本标签>.zip`（例如 `wayx-windows-x64-v0.1.0.zip`），
不重新编译。

## 项目结构

| 目录 | 内容 |
| --- | --- |
| `src/` | Rust 显示后端、窗口管理、渲染、输入法、剪贴板与 SSH |
| `ddx/`、`include/` | XLibre DDX 与版本化 C ABI |
| `tests/` | Rust 集成测试及 C/C++ 协议客户端 |
| `scripts/` | 构建、打包与测试工具 |

线程模型和组件关系见 [架构说明](docs/architecture.md)。

## 许可

自有代码使用 MIT。XLibre、Pixman 及其他依赖保留各自许可，分发时须遵守运行包
`licenses` 中的对应许可。
