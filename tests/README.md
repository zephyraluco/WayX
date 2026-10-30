# 集成测试结构

这里的测试覆盖不同的边界；C/C++ 客户端不替代 Rust 测试，也不应仅因文件名相近而合并。

| 文件 | 验证职责 | 运行入口 |
| --- | --- | --- |
| `local_display.rs` | 默认免认证和 Xauthority 模式下的内部 WM/XIM 服务 | 显式运行 ignored 测试，`WAYX_TEST_LOCAL_AUTH=1` 选择认证模式 |
| `xlibre.rs` | 真实 X11 绘制、输入、剪贴板、滚动、RandR 与 GLX 协商 | 显式运行 ignored 测试 |
| `rootless.rs` | 遮挡后的独立窗口像素、标题/几何通知与窗口生命周期 | 同上 |
| `window_manager.rs` | ICCCM/EWMH、窗口约束、焦点与协作关闭 | 同上 |
| `desktop_layout.rs` | 宿主多屏坐标、指针边界、热插拔与拖动期间的 RandR 行为 | 同上 |
| `ime.rs` + `xim-client.c` | 真实 Xlib XIM 协议、Unicode 提交与预编辑 | `WAYX_TEST_XIM_CLIENT` 指定 C 客户端 |
| `glx.rs` + `glx-client.c` / `ogre-client.cpp` | TCP 软件 OpenGL / OGRE 绘制及 DDX 实际像素 | `WAYX_TEST_GLX_CLIENT` 指定客户端 |
| `native_geometry.rs` / `native_gpu.rs` | 通过带认证的 TCP 连接，验证原生窗口几何与共享 GPU 资源生命周期 | `scripts/test-idle-cpu.py --geometry --gpu-cycles` |
| `native-resize.c` / `native-rootless.c` / `native-glx.c` | 从 Linux 宿主 X11 观察最终呈现、原生窗口关系、独立关闭与像素 | `scripts/test-native.py` |
| `idle-client.c` | 保持静止窗口并按命令制造 Damage，检查空闲后能恢复呈现 | `scripts/test-native.py` |
| `bridge.c` | 用 C 编译器验证 ABI 布局、回调、缓冲区边界与剪贴板 | `scripts/test-c-bridge.sh` |
| `scale-client.c` | 实际宿主像素、指针映射、尺寸约束与双向缩放 | `scripts/test-scale.py`，覆盖 rootful/rootless 与固定/自动倍率 |

`support/core.rs` 集中管理嵌入式 XLibre 的动态库、C 参数、API/Handle 生命周期、
5 秒关闭超时和异常退出清理。各测试仍显式选择显示编号、传输方式、屏幕参数与回调。
一个进程只能运行一次 XLibre，故协议测试保留为独立 Cargo 集成测试目标。

`support/x11.rs` 统一原生几何/GPU 客户端的 Xauthority 认证 TCP 连接。
两个公共模块通过路径导入，不会被 Cargo 当成独立测试目标。
其余等待循环涉及不同的协议泵送或断言条件，保留在对应测试中。

从仓库根目录运行协议测试（先构建核心和 XIM/GLX 客户端）：

```sh
export WAYX_TEST_CORE="$(cat build/xlibre/wayx-core-path.txt)"
export WAYX_TEST_XIM_CLIENT=/path/to/xim-client
export WAYX_TEST_GLX_CLIENT=/path/to/glx-client
cargo test --locked --test xlibre --test rootless --test window_manager \
  --test desktop_layout --test ime --test glx -- --ignored --nocapture
```

外部 OpenSSH 转发在 Linux 运行以下命令验证：

```sh
python3 scripts/test-openssh.py --binary target/debug/wayx \
  --core build/xlibre/hw/wayx/libwayx-xserver.so
```

需要 OpenSSH 客户端/sshd、xauth、
Xvfb、xdpyinfo、xwininfo 和 X11 C 开发环境。测试使用临时密钥、独立 sshd 和显示编号，
在默认免认证及 Xauthority 两种模式下检查真实宿主像素和 IPv4/IPv6 回环监听。
Windows 的 PuTTY 和 macOS 的 OpenSSH 需要对应原生环境验证。

缩放测试在 Linux 用 Xvfb 执行 0.5×、1×、1.25×、1.5×、2×、4× 及模拟 2× DPI 的 auto；
运行包测试使用预编译 `scale-client`。原有原生几何基线显式使用 `--scale 1`。
真实混合 DPI 跨屏、Wayland、Windows 和 Retina 交互仍需对应宿主的原生验证。

macOS 运行包测试会执行 `scripts/test-macos-lifecycle.py`：检查真实 AppKit 激活策略，
依次打开两个窗口、关闭最后一个窗口、再次打开并关闭窗口，确认隐藏后仍可连接 X11。
显隐延迟、弹出窗口、最小化窗口和快速重建逻辑由 `app_lifecycle` 单元测试覆盖。

默认 `cargo test` 会跳过需要核心/原生窗口的测试，不能据此声称它们已通过。
CI 用 `scripts/build-test-kit.py` 编译夹具，再由 `scripts/test-package.py` 在迁移后的
运行包上执行；打包测试必须使用预编译夹具，不允许回退到源码构建。
