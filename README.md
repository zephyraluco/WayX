# WayX

按 **XLibre X Server Core + C DDX Bridge + Rust + winit + wgpu + russh** 架构实现的 X11 转发客户端。

当前版本支持 Linux 上的 **rootful / rootless 显示**：远端 X11 应用通过 SSH 标准 X11 forwarding 接入本地 XLibre；XLibre FB/Pixman 生成的软件帧缓冲经 Damage 上传到 Rust，最终由 wgpu 呈现在 winit 原生窗口中。

**不是完整的跨平台 X Server 发行版。** Rust 显示后端面向 Win32 / AppKit / Linux Wayland、X11；XLibre DDX 已在 Linux、macOS 和 Windows 构建及运行验证。已支持逐 X11 顶层窗口的 rootless 映射；Linux X11 窗口管理已接入；已接入原生 IME → XIM 文本提交及回调式预编辑，三平台输入法交互仍待验收。Linux 使用 Wayland 作为本地窗口系统，不等于支持远端原生 Wayland 协议转发。

## 工程结构

| 路径 | 作用 |
| --- | --- |
| `src/backend.rs` | 窗口生命周期、父子关系、焦点、帧缓冲、Damage、输入队列、剪贴板状态 |
| `src/bridge.rs`, `include/wayx.h` | 版本化 C ABI、回调表、缓冲区校验、线程同步 |
| `ddx/` | XLibre 软件 framebuffer DDX、Damage、键鼠输入接入 |
| `src/desktop.rs` | winit 原生窗口/事件、wgpu 纹理上传与呈现、本地剪贴板 |
| `src/ime.rs` | `@im=wayx` XIM 服务、输入焦点路由、Unicode 提交及预编辑回调 |
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

## macOS 与 Windows 核心

macOS 的 XLibre 核心、真实协议测试和 AppKit/Metal rootful 窗口已在原生 CI 通过；运行包搬移后的启动也已通过；逐窗口的 AppKit 交互仍需专门验收。

```sh
brew install meson ninja pkgconf pixman libxfont2 libxkbfile xorgproto xtrans xkbcomp xkeyboard-config
bash scripts/build-xlibre.sh
cargo build --locked
WAYX_TEST_CORE="$(cat build/xlibre/wayx-core-path.txt)" \
  cargo test --locked --test xlibre --test rootless --test window_manager -- --ignored --nocapture
```

构建脚本将实际 `.so` / `.dylib` / `.dll` 路径写入 `build/xlibre/wayx-core-path.txt`，`serve --core` 使用该路径。XKB 路径通过原生 pkg-config 获取；`WAYX_XKB_DIR` 与 `WAYX_XKB_BIN_DIR` 可在构建和运行时覆盖。

Windows 使用固定版本 vcpkg 编译 C 依赖，再用 MSYS2 UCRT64 构建协议头、XKB 工具、数据和核心 DLL；Rust 使用 MSVC，通过版本化 C ABI 交互。步骤见 [原生构建工作流](.github/workflows/native-core.yml) 与 [Windows 构建脚本](scripts/build-windows-core.sh)。核心 DLL、三项真实 X11 协议测试、Win32/DX12 呈现和含空格目录中的运行包重定位已在原生 CI 通过（提交 `a496bce`，[验证记录](https://github.com/zephyraluco/WayX/actions/runs/39709583993)）。这仍是实验性运行包，逐窗口原生交互及正式发行验收不在此验证范围内。

`scripts/bundle-core.py` 整理客户端、核心、动态库依赖、XKB 数据和许可证；macOS 修正加载路径并使用 ad-hoc 签名，Windows 生成 CMD 启动入口。CI 移动运行包后启动，Windows 还隐藏原始依赖目录。这是运行包验证，不代表正式安装器或可信发行签名已经完成。

## macOS 运行包快速连接

从 [Native XLibre core 构建记录](https://github.com/zephyraluco/WayX/actions/workflows/native-core.yml)
下载 `macos-core-runtime-<提交号>`（完整运行包，而非仅包含客户端的构件）。
解压 ZIP，再解压里面的 `macos-core-runtime.tar.gz`，进入 `relocated-runtime`。
运行包的二进制架构需与本机兼容，可用 `file bin/wayx` 和 `uname -m` 检查。
不需要 XQuartz。

当前仅有 ad-hoc 签名，没有 Developer ID 签名和 Apple 公证。遇到“Apple 无法验证”
提示，可在“系统设置 → 隐私与安全性”中允许打开。确认是本项目下载的运行包后，
也可仅在该运行包目录内执行 `xattr -dr com.apple.quarantine .`。

以下认证文件只创建一次；不要在本地服务运行期间重新生成：

```sh
cd /你的路径/relocated-runtime
chmod +x wayx.sh bin/wayx bin/xkbcomp
if [ ! -e Xauthority ]; then
  umask 077
  cookie="$(openssl rand -hex 16)"
  printf 'ffff0000000331303000124d49542d4d414749432d434f4f4b49452d310010%s' "$cookie" |
    xxd -r -p > Xauthority
  unset cookie
fi
./wayx.sh --rootless -- :100 -screen 0 1280x800x24 -crtcs 1@1280x800 \
  -nolisten tcp -auth "$PWD/Xauthority"
```

保持服务运行，在另一终端进入同一个运行包目录后连接：

```sh
./bin/wayx ssh --host 172.22.2.23 --user jz --password \
  --known-hosts "$HOME/.ssh/known_hosts" --xauthority "$PWD/Xauthority" \
  --display 100 --command 'wjtools-arm64'
```

主机公钥须已核实并记录在 `known_hosts`，远端须启用 SSH X11 forwarding 并安装
`xauth`。rootless 启动时不会立即显示根桌面；远端程序打开窗口后才会出现本地窗口。

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

默认单个原生窗口承载整个 X 屏幕；内部 X 窗口由 XLibre 管理。需要桌面装饰时，可在该显示中启动自己选择的 X11 窗口管理器。拖动原生窗口尺寸当前只缩放画面，不改变 X 屏幕分辨率。

## Rootless：每个 X11 顶层窗口独立显示

在上述 `serve` 命令的 `--` 之前添加 `--rootless`：

```sh
cargo run --locked -- serve --rootless --core build/xlibre/hw/wayx/libwayx-xserver.so -- \
  :100 -screen 0 1280x800x24 -crtcs 1@1280x800 -auth .cache/Xauthority -nolisten tcp
```

顶层窗口使用 XLibre Composite 的独立 Pixmap，因此互相遮挡也能保留正确像素；嵌套子窗口合成到所属顶层窗口。支持标题更新、映射/隐藏、窗口销毁和客户端尺寸变化；根桌面保持隐藏。每个原生窗口的键鼠输入按对应的 X11 窗口路由。

rootless 内置窗口管理器取得 `WM_S0` 和 SubstructureRedirect，处理 map/configure、激活、层级、关闭及常用 EWMH 状态请求；不要同时运行其他 X11/Composite 窗口管理器。移动和缩放双向同步，`WM_NORMAL_HINTS` 约束尺寸，关闭优先通知 `WM_DELETE_WINDOW`，不支持该协议的应用使用 XKillClient 语义（同一客户端的其他窗口也会关闭）。override-redirect 弹出窗口无装饰且不主动获取焦点；X11 宿主建立原生 WM_TRANSIENT_FOR 关系。

当前仍限定软件 32-bpp XRGB 显示，不支持透明 ARGB。窗口管理已在 Linux X11 宿主实际运行验证；Wayland 限制应用控制全局窗口位置，winit 也未提供通用的 Wayland/macOS 瞬态父窗口接口，这些平台行为尚未验收。C ABI 已升级至 **v2**，旧插件必须重新编译；新建窗口先隐藏，再由 DDX 显式 map。

## SSH X11 forwarding

保持本地 `serve` 运行。远端 sshd 须启用 `X11Forwarding yes` 并安装 `xauth`。本地主机的 `known_hosts` 必须已经通过可信途径记录远端主机公钥。

```sh
cargo run --locked -- ssh \
  --host example.org --port 22 --user alice \
  --identity /path/to/id_ed25519 --known-hosts /path/to/known_hosts \
  --xauthority .cache/Xauthority --display 100 --command 'xterm'
```

密码登录可直接使用内置 SSH 客户端，无需其他 SSH 软件。在 Windows 的另一个
PowerShell 中执行（保持 `wayx.cmd` 的本地服务运行）：

```powershell
.\bin\wayx.exe ssh --host 172.22.2.23 --user jz --password `
  --known-hosts "$env:USERPROFILE\.ssh\known_hosts" `
  --xauthority .\Xauthority --display 100 --command "wjtools-arm64"
```

验证服务器主机密钥后，终端提示输入账号密码，输入不回显。省略 `--identity`
时默认使用密码登录；`--password` 与 `--identity` 互斥，不提供明文密码命令行参数。
`--key-passphrase-env` 仅用于私钥解密，需要同时指定 `--identity`。服务器必须
允许 SSH password 认证；keyboard-interactive/PAM 多轮交互尚未实现。

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
WAYX_TEST_CORE=$PWD/build/xlibre/hw/wayx/libwayx-xserver.so \
  cargo test --locked --test rootless -- --ignored --nocapture
WAYX_TEST_CORE=$PWD/build/xlibre/hw/wayx/libwayx-xserver.so \
  cargo test --locked --test window_manager -- --ignored --nocapture
cargo run --locked -- gpu-smoke
```

默认测试包括本地真实加密 SSH 会话，不需要任何远端凭据。XLibre 集成测试必须显式运行；测试会建立隔离的 `:191` Unix 显示、禁用 TCP，并在测试结束时关闭服务。它验证真实 X11 绘制、键盘/鼠标回传、中文剪贴板双向转换和大文本 INCR 双向传输；不以打开端口或零测试运行作为成功依据。

三平台 CI 已配置，但工作流必须在 GitHub 上运行后才能确认原生结果。当前实现边界与线程模型见 [架构说明](docs/architecture.md)。

## 许可

本仓库自有代码使用 MIT。XLibre、Pixman 及其他依赖保留各自许可；XLibre 按文件包含不同许可，分发其源码或二进制时须单独遵守。构建脚本从上游获取代码，不将其声明为本仓库自有代码。

后续功能按 [实施进度](docs/progress.md) 逐项完成验证并独立提交。

### CI 构建产物下载

每次推送都会构建 Windows、macOS、Linux 的 release 客户端，并运行 XLibre
核心验证。成功构建的客户端、Linux 核心和通过验证的 macOS/Windows 核心运行包
可在对应 [Actions 运行记录](https://github.com/zephyraluco/WayX/actions) 的
Artifacts 区域下载，保留 **7 天**；名称包含平台、架构（客户端）和完整提交 SHA。
客户端单独下载包不包含 XLibre 及其运行依赖；核心运行包仍为实验性发行。
构建或验证失败的平台不会发布成功产物，具体以对应任务结果为准。

## 输入法（IME/XIM）

使用带 `-auth` 的正常桌面启动方式时，WayX 在自己的 XLibre display 上自动注册
`@im=wayx`，无需在远端运行额外的输入法守护进程。本地输入法由宿主系统提供。
远端应用须使用 XIM，而不是独立的 IBus/Fcitx 或 Wayland 输入法通道。例如 Windows：

```powershell
.\bin\wayx.exe ssh --host 172.22.2.23 --user jz --password `
  --known-hosts "$env:USERPROFILE\.ssh\known_hosts" `
  --xauthority .\Xauthority --display 100 `
  --command "env XMODIFIERS=@im=wayx GTK_IM_MODULE=xim QT_IM_MODULE=xim wjtools-arm64"
```

远端需要可用的 UTF-8 locale（用 `locale` 检查）；GTK/Qt 还需安装对应的 XIM
输入模块。是否采用上述变量取决于应用所用工具包。未创建 XIC 的应用仍只能使用
普通物理键输入。SSH 转发会自动传输 XIM 使用的 X11 请求，不需要额外端口。

已验证真实 Xlib 的 `XOpenIM`、`XCreateIC`、`XFilterEvent`、`Xutf8LookupString`，
包含中日韩、emoji、超过旧 32 字节限制的提交、预编辑回调、光标坐标、窗口切换、
reset、销毁及异常退出清理。单次文本上限为 8 KiB，超限明确拒绝。
候选窗位置按 XIC spot location 转换到原生窗口坐标；没有 spot 的客户端使用窗口原点。
回调式预编辑由远端应用显示；`PreeditNothing`/`PreeditPosition` 使用 WayX 本地
预编辑层，显示组合文字、下划线及光标。字体来自宿主系统，需有相应中日韩字体；
运行包不附带字体。预编辑层最多显示两行，长文本会裁剪，单次提交仍支持 8 KiB。
Windows/macOS/Wayland 的真实系统输入法、候选窗及组合键交互仍需原生人工验收；
此阶段不将第 4 项记为完整跨平台验收完成。

Linux 真实 XIM 回归（需先构建 XLibre）：

```sh
cc -std=c11 -Wall -Wextra -Werror tests/xim-client.c -lX11 -o /tmp/wayx-xim-client
WAYX_TEST_CORE=$PWD/build/xlibre/hw/wayx/libwayx-xserver.so \
WAYX_TEST_XIM_CLIENT=/tmp/wayx-xim-client \
cargo test --locked --test ime -- --ignored --nocapture
```

## 平滑滚动

触控板的小数滚动通过 XI2 连续滚动轴传给应用；只支持旧式滚轮的应用由 XLibre 累积后接收按钮 4–7。支持水平、垂直、方向反转以及操作系统产生的惯性滚动，不额外合成惯性。真实核心测试验证两轴小数、反转、旧客户端按钮数量和指针位置不变。Windows/macOS 物理触控板手感仍需人工验收。
