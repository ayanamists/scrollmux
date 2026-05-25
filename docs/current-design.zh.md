# ScrollMux 当前设计与实现速览

这份文档面向有编程语言和前端开发经验、但还没有写过 TUI 程序的读者。目标不是解释所有终端细节，而是给出理解当前源码所需的最小上下文。

## 一句话模型

ScrollMux 更像一个很小的“终端组合器”，不是传统意义上的 widget TUI。

它维护多个 PTY 终端会话，每个会话占据一个固定宽度的 pane；宿主终端窗口只是横向工作区上的一个 viewport。

```text
虚拟横向工作区:
[ pane0: 120 列 ][ pane1: 120 列 ][ pane2: 120 列 ][ pane3: 120 列 ]

宿主终端窗口:
                 └──────── viewport_x .. viewport_x + screen_width ────────┘
```

核心不变量：pane 的逻辑宽度独立于宿主终端宽度。新增 pane 不会挤压已有 pane；移动 viewport 不会 resize PTY；宿主终端 resize 通常只影响 pane 高度。

## 代码入口

- `src/main.rs`：进程入口。读取宿主终端尺寸，安装 raw mode 和 alternate screen，创建 `App`，退出时恢复终端。
- `src/app.rs`：应用胶水层。拥有 `Workspace`，运行事件循环，处理输入、resize 和 tick，触发渲染，关闭 PTY。
- `src/workspace.rs`：核心状态和布局数学。这里决定 focus、viewport、pane 顺序、可见区域裁剪。
- `src/pane.rs`：单个 pane。负责启动 PTY、维护 `vt100::Parser`、接收输出、写入输入。
- `src/render.rs`：把 pane 的 `vt100` 屏幕缓冲按 viewport 裁剪后绘制到宿主终端。
- `src/input.rs`：用 `termwiz` 解析宿主 stdin 字节，只识别 ScrollMux 快捷键；其他输入保留原始 bytes 转发给当前 PTY。
- `src/stdin_pump.rs`：后台线程读取宿主 stdin 原始 bytes，送入主循环。
- `src/signal_pump.rs`：监听 SIGWINCH，把宿主终端 resize 送入主循环。

## 依赖的分工

- `crossterm`：控制宿主终端，包括 raw mode、alternate screen、光标移动、颜色和属性输出。
- `termwiz`：把宿主 stdin 原始 bytes 解析成语义输入事件，仅用于判断是否命中 mux 快捷键。
- `signal-hook`：监听 SIGWINCH。
- `portable-pty`：创建真正的伪终端和子进程，例如用户的 shell。
- `vt100`：解析子进程输出里的 ANSI/VT escape sequence，维护一个终端屏幕模型。

因此 ScrollMux 不手写终端模拟器。它做的是：接收 PTY 字节流，交给 `vt100` 解析，再把 `vt100` 当前屏幕内容复制到宿主终端。

## 运行时数据流

1. `main` 调用 `TerminalGuard::install()`，进入 raw mode 和 alternate screen。
2. `App::run()` 启动一个默认 shell pane。
3. 每个 pane 创建一个 PTY，并启动 reader thread。
4. reader thread 从 PTY master 读字节，喂给 `vt100::Parser`，然后把 pane 标记为 dirty。
5. `stdin_pump` 线程读取宿主 stdin 原始 bytes，`signal_pump` 监听 resize，16ms tick 驱动主循环醒来。
6. 输入 bytes 进入 `input::InputAccumulator`：
   - `termwiz::InputParser` 只负责解析语义事件，判断是否是单个 ScrollMux 快捷键。
   - 如果是单个 mux 快捷键，变成 `Action`，例如切换 focus、创建 pane、移动 viewport。
   - 否则原始 bytes 不经重新编码，直接写入当前 focused pane 的 PTY。
7. 如果有 pane dirty，`render::render()` 清屏并重绘当前 viewport 内可见的 pane 区域和底部状态栏。

简化成一条链路：

```text
用户输入 bytes
  -> stdin_pump
  -> termwiz InputParser + input::classify
  -> App::handle_action
  -> Workspace 状态变化，或 Pane::write_input

PTY 输出
  -> reader thread
  -> vt100::Parser
  -> dirty = true
  -> render::render
  -> 宿主终端画面
```

## Workspace 是核心模型

`Workspace` 当前接近这个形状：

```rust
pub struct Workspace {
    pub panes: Vec<Pane>,
    pub focused: usize,
    pub viewport_x: u32,
    pub screen_width: u16,
    pub screen_height: u16,
    pub default_width: u16,
}
```

几个重要字段：

- `panes`：横向排列的所有 pane。
- `focused`：当前接收输入的 pane index。
- `viewport_x`：宿主窗口左边缘在虚拟横向工作区里的 x 坐标。
- `screen_width` / `screen_height`：宿主终端尺寸。
- `default_width`：新建 pane 的默认逻辑宽度。

和前端类比，`Workspace` 类似应用状态加布局计算；但这里布局单位不是 CSS pixel，而是终端 cell。

## 可见区域裁剪

最关键的布局函数是 `Workspace::visible_panes()`。

它遍历所有 pane，计算每个 pane 在虚拟工作区里的 `[start, end)`，再和当前 viewport `[viewport_x, viewport_x + screen_width)` 求交集。

返回值是 `VisiblePane`：

```rust
pub struct VisiblePane {
    pub idx: usize,
    pub src_left: u16,
    pub src_right: u16,
    pub dst_left: u16,
}
```

含义：

- `idx`：对应 `Workspace::panes` 中哪个 pane。
- `src_left` / `src_right`：pane 内部哪些列可见。
- `dst_left`：这些可见列应该画到宿主终端的哪一列。

这就是 ScrollMux 的“横向相机”机制。渲染器只需要按这个列表把 pane 内部的屏幕内容裁剪复制出来。

## Pane 是一个 PTY-backed 终端

`Pane` 保存：

- `name`：状态栏显示名。
- `width` / `height`：这个 pane 的逻辑终端尺寸。
- `parser: Arc<Mutex<vt100::Parser>>`：终端屏幕状态。
- `dirty: Arc<AtomicBool>`：后台 reader thread 通知主线程需要重绘。
- `pty`：PTY master、writer、child process。

启动 pane 时：

1. `portable-pty` 创建 PTY pair。
2. 在 slave 端启动 shell 命令。
3. master 端 clone 一个 reader，拿一个 writer。
4. reader thread 持续读取输出并更新 `vt100::Parser`。

输入路径相反：宿主 stdin 原始 bytes 只有在命中单个 mux 快捷键时被消费；其他 bytes 通过 `Pane::write_input()` 原样写入 PTY writer。

## 渲染模型

`render::render()` 是 immediate-mode 风格：

1. 隐藏光标，重置颜色。
2. 清空宿主终端屏幕。
3. 调 `ws.visible_panes()` 得到当前可见 pane。
4. 对每个可见 pane，锁住它的 `vt100::Parser`，读取 `screen()`。
5. 对每一行、每一列，从 `src_left..src_right` 复制 cell 到宿主终端的 `dst_left`。
6. 根据 cell 的颜色、粗体、斜体、下划线、反色设置 `crossterm` attribute。
7. 绘制底部状态栏。
8. 如果 focused pane 的 cursor 在 viewport 内，把宿主终端光标移动到对应位置并显示。

这里没有 retained widget tree，也没有 diff DOM。当前实现每次重绘会清整屏，再绘制当前 frame。

## 输入模型

输入层做两类事：

1. `termwiz` 把当前 stdin bytes 片段解析成语义事件。如果这个片段只包含一个 mux 快捷键，则解释为 ScrollMux 命令，例如：
   - `Alt-h` / `Alt-l`：切换 focus。
   - `Alt-n`：新建 pane。
   - `Alt-w`：关闭当前 pane。
   - `Alt-[` / `Alt-]`：水平移动 viewport。
   - `Alt-=` / `Alt--`：调整当前 pane 宽度。
2. 其他输入全部透传原始 bytes，包括普通字符、Enter、Backspace、方向键、paste markers、鼠标报告、IME/UTF-8 输入、kitty keyboard 协议和 OSC52。

所以 ScrollMux 的输入处理不是“解码后再编码”，而是“拦截自己的快捷键；其他像真实终端一样透传给 PTY”。

注意：`Alt-*` 快捷键依赖宿主终端把 Option/Alt 发成 ESC-prefix Meta 输入。macOS Terminal.app 需要打开 "Use Option as Meta key"；iTerm/Ghostty/WezTerm 通常默认支持或有等价设置。

## 当前实现边界

已经有的能力：

- 单 workspace。
- 多个固定宽度 pane。
- 每个 pane 一个 PTY。
- focus 前后切换。
- 新建、关闭、移动 pane。
- viewport 左右滚动。
- 调整 focused pane 宽度。
- 宿主终端 resize 时保留 pane 宽度，只更新高度。
- 底部状态栏。

还没有的能力：

- 配置文件和 session restore。
- daemon / attach / detach。
- 多 workspace 或 tab。
- 嵌套 split tree。
- 完整 copy mode。
- 鼠标交互。
- 插件系统。
- 复杂主题系统。

这和项目原则一致：先把“固定宽度 PTY 列 + 横向 viewport”这个核心做稳。

## 阅读源码的建议顺序

1. 先读 `src/workspace.rs`，理解 `visible_panes()` 和宽度不变量。
2. 再读 `src/render.rs`，看可见 pane 如何被复制到宿主终端。
3. 然后读 `src/pane.rs`，理解 PTY、reader thread、`vt100::Parser` 的关系。
4. 最后读 `src/app.rs`、`src/stdin_pump.rs`、`src/signal_pump.rs` 和 `src/input.rs`，把事件循环和快捷键串起来。

只要理解了这四个概念，当前实现基本就通了：

- pane 是固定宽度的虚拟终端。
- workspace 是横向 strip。
- viewport 是宿主终端窗口。
- renderer 是把 strip 的可见部分投影到宿主终端。
