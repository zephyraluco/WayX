# 集成测试结构

这里的测试覆盖不同的边界；C/C++ 客户端不替代 Rust 测试，也不应仅因文件名相近而合并。

| 文件 | 验证职责 | 运行入口 |
| --- | --- | --- |
| `ssh.rs` | 本地加密 SSH、认证、压缩、cookie 替换与退出状态 | `cargo test --locked --test ssh` |
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

默认 `cargo test` 会跳过需要核心/原生窗口的测试，不能据此声称它们已通过。
CI 用 `scripts/build-test-kit.py` 编译夹具，再由 `scripts/test-package.py` 在迁移后的
运行包上执行；打包测试必须使用预编译夹具，不允许回退到源码构建。
