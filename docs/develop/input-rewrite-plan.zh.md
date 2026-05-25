# Input 层重构 Plan:摆脱 crossterm event 的 byte→struct→byte round-trip

## TL;DR

现状是宿主 stdin 字节 → `crossterm::event::read()` → `KeyEvent`/`Event::Paste` → `encode_for_pty` 反向序列化回字节 → 写 PTY。这次解码-编码毫无信息增量,而且 `Event::Paste` 在 `app.rs:handle_event` 里被静默丢掉,导致 macOS 上 cmd+v 不工作。

目标改造:**像 zellij 一样,语义事件 + 原始字节并排传递,在 dispatch 那一刻决定走 mux 还是透传**。Paste / 鼠标 / IME / kitty 协议 / OSC52 全部白嫖。

参考实现:zellij 的 `zellij-client/src/stdin_handler.rs` + `zellij-server/src/route.rs` + `zellij-server/src/tab/mod.rs`(本地 clone 在 `~/repo/zellij`)。

## 目标 / 非目标

**目标**
- 删除 `src/input.rs::encode_for_pty` 整张反向映射表。
- 宿主 stdin 字节直送 focused pane PTY,只在 mux 快捷键处拦截。
- Paste / 鼠标 / IME / kitty 协议自动可用。

**非目标**
- 不引入 mode 系统(zellij 的 normal/pane/tab 多模式键位)。
- 不做 client/server 拆分。
- 不重写渲染层。
- 不实现终端 emulator(沿用 `vt100::Parser` 处理 PTY 输出)。

## 架构

```text
host stdin (raw bytes)
        │
        ▼
   ┌─────────────────┐
   │ stdin_pump 线程 │  read(4096) → mpsc
   └─────────────────┘
        │
        ▼  Vec<u8> 片段
   ┌─────────────────────────────────┐
   │ main loop                       │
   │  recv_timeout(50ms)             │ ← escape-time
   │  buffer.extend(chunk)           │
   │  parser.parse(buf, cb,          │
   │    maybe_more=!timeout)         │ ← termwiz InputParser
   │  events: Vec<InputEvent>        │
   │  ┌────────────────────────────┐ │
   │  │ if events.len()==1 &&      │ │
   │  │    is_mux_shortcut(&e[0]): │ │
   │  │     consume → mux action   │ │
   │  │ else:                      │ │
   │  │     focused.write(&buf)    │ │ ← 零拷贝透传(paste markers 已含)
   │  └────────────────────────────┘ │
   │  buf.clear()                    │
   └─────────────────────────────────┘
```

并入同一个 mpsc 的另外两路:
- SIGWINCH (signal-hook) → `HostEvent::Resize(cols, rows)`
- 16ms tick → `HostEvent::Tick`(驱动 dirty-repaint)

## Cargo.toml 改动

```toml
# 新增
termwiz = { version = "0.23", default-features = false }   # InputParser only
signal-hook = "0.3"                                         # SIGWINCH

# crossterm 保留:raw_mode / alt_screen / cursor / 字符串 print
# 不再使用 crossterm::event 子模块
```

> termwiz 默认 feature 拉得多,关掉 `default-features` 只用 input parser。如果实测依赖树仍嫌重,fallback 是 `vte 0.13` + 自写 ~100 行最小 KeyEvent 层。

## 文件级改动清单

### `src/input.rs` —— 重写

- 删 `Action::Input(Vec<u8>)` 之外的 `encode_for_pty` 整张反向映射表(~60 行)。
- 新签名:
  ```rust
  pub enum Action {
      Quit, FocusPrev, FocusNext, MoveLeft, MoveRight,
      NewPane, CloseFocused, Center,
      ScrollLeft, ScrollRight, GrowWidth, ShrinkWidth,
      Input(Vec<u8>),   // 直接来自 stdin 缓冲,不经任何编码
  }
  pub fn classify(ev: &termwiz::input::InputEvent) -> Option<Action>;
  ```
- `classify` 只识别 mux 快捷键 (Alt+h/l/H/L/n/w/f/[/]/=/-/+/_/q),其它一律 `None`(意味着调用方走透传分支)。

### `src/stdin_pump.rs` —— 新文件 (~40 行)

```rust
pub fn spawn(tx: mpsc::Sender<HostEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(HostEvent::Bytes(buf[..n].to_vec())).is_err() { break; }
                }
                Err(_) => break,
            }
        }
    })
}
```

### `src/signal_pump.rs` —— 新文件 (~25 行)

signal-hook 监听 SIGWINCH,每次触发拿 `crossterm::terminal::size()` 推 `HostEvent::Resize(cols, rows)`。

### `src/app.rs` —— event loop 改写

- 删 `event::poll/read` 那段(当前 `app.rs:60-72`)。
- 新结构:
  ```rust
  enum HostEvent { Bytes(Vec<u8>), Resize(u16, u16), Tick }
  ```
- 主循环用 `rx.recv_timeout(Duration::from_millis(16))`,16ms 同时驱动 tick 与 escape-time finalize。
- escape-time 双保险(zellij 风格):
  1. `recv_timeout(50ms)` 提供时间侧。
  2. termwiz `parse(.., maybe_more)`:`maybe_more=true` 当 `recv_timeout` 拿到 `Bytes`(可能还有后续);`false` 当超时(已知没续行,残留 ESC 当裸 Esc)。
- 命中 mux 分支调原有 `handle_action`,**不**调 `write_input`。
- 透传分支:`self.ws.panes[focused].write_input(&stdin_buf)`。
- `TerminalGuard` 保留 `EnableBracketedPaste`(让宿主送 `\e[200~`/`\e[201~`)。透传时这些标记原样进 PTY,inner app 自决。

### `src/pane.rs`

不动。`write_input` 已是 `&[u8]` 接口。

### `src/main.rs`

仅加 `mod stdin_pump; mod signal_pump;`。

### `src/render.rs` / `src/workspace.rs`

**完全不动**。

## 边界 case 决策表

| 情况 | 行为 | 理由 |
|---|---|---|
| 用户单按 Alt+h | 1 个 KeyEvent → mux 命中 → 消费 | 主路径 |
| 用户输入 `ls` | 2 个 KeyEvent → 都不命中 → 整 buf 写 PTY | 透传 |
| 粘贴 `\e[200~hello\e[201~` | 1 个 `Paste("hello")` → 不命中 → 整 buf 写 PTY(含 markers) | inner app 直接获得 bracketed paste,无需我们 re-wrap |
| Alt+h 紧跟着 `ls`(同一个 read) | 多事件,有一个是 mux | **整 buf 透传**,放弃消费 mux —— 避免把 `ls` 误丢 |
| 裸 ESC | recv_timeout 触发,`maybe_more=false`,termwiz 产 `Esc` KeyEvent → 不命中 → 写 PTY `\x1b` | 正常 |
| 鼠标 / kitty 协议 / OSC52 | termwiz 解析成对应事件,都不命中 → 整 buf 写 PTY | 白嫖 |

"多事件含 mux 时全透传" 是个**保守选择**。zellij 选了"每个事件单独 dispatch、raw_bytes 只挂第一个"的方案,代价是快速连按时丢字节;我们选另一面。实际打字模式观察后再调。

## 测试矩阵

### 单元测试 (`src/input.rs::classify`)
- 每个 mux 快捷键对应的 `InputEvent::Key { key: Char('h'), modifiers: ALT }` 返回正确 `Action`。
- Shift 修饰符 (`Alt+Shift+h` → `MoveLeft`)。
- 非快捷键返回 `None`。

### 集成层(包装函数 `parse_then_dispatch` 之类)
- 字节序列 `[0x1b, 'h']` + `maybe_more=false` → `Action::FocusPrev`,无透传字节。
- 字节序列 `[0x1b]` + `maybe_more=false` → 透传 `\x1b`。
- 字节序列 `\e[200~hello\e[201~` → 透传整段。
- 字节序列 `[0x1b, '[']` + `maybe_more=true` → 不产事件,不动作(等续行)。
- UTF-8 多字节(如 `é` = `0xc3 0xa9`)→ 透传两字节。

不写 perf 测试:透传本身比现在快(少一次 encode),不会回归。

## 分步落地(建议 4 次 commit)

1. **加 deps + stdin_pump + signal_pump**,但 app loop 不切换,只验证 pump 能拿到字节(临时 log 到文件)。`main` 仍跑老 loop。
2. **接入 termwiz 解析**,在老 loop 旁边并行解析,把结果和 crossterm 的对比 log。验证 escape-time 与 maybe_more 工作正常。
3. **切换主路径**:`app.rs` event loop 改成 mpsc;`input.rs` 重写 `classify`,删 `encode_for_pty`。这一 commit 是核心,改动集中、可单独 revert。
4. **清理 + 文档**:更新 `docs/invarints.md` 加 input 不变量("mux shortcuts are the only consumed events; all other bytes reach focused PTY verbatim"),更新 `CLAUDE.md` "Known input gap: bracketed paste …" 那段为 resolved。

## 风险 / 开放问题

1. **termwiz 体积**:`default-features = false` 之后的依赖树要 `cargo tree` 实测。过重则 fallback 到 `vte` + 自写 key parser。建议在 commit 1 之前做 spike。
2. **stdin 与 crossterm 的所有权**:确保改造后不再调任何 `crossterm::event::*`。crossterm 在 `terminal::size()` / `enable_raw_mode()` / `execute!()` 这些纯 ioctl 路径上不读 stdin,共存安全。
3. **panic 安全**:`TerminalGuard` 继续管 `EnableBracketedPaste`;stdin_pump 线程在主进程 panic 时被连带杀掉,正常。
4. **macOS Option-as-Meta**:Terminal.app 默认 Option **不**发 ESC 前缀,需要用户在终端设置里开 "Use Option as Meta key"。iTerm/Ghostty/Wezterm 默认对。旧实现也有同样问题,不算回归,但 README 应加一行说明。
5. **Bracketed paste 中嵌入 mux 快捷键**:粘贴文本里恰好含 `\e h`?termwiz 的 `Paste(String)` 把整段视为一个事件,不会拆成多个 KeyEvent,**不会**误触发 mux —— 走透传分支,markers 包住整段,inner app 看到正常 paste。这一条是 termwiz 路线相对纯字节前缀树的明确优势,值得在 commit message 强调。

## 参考

- zellij stdin pump: `~/repo/zellij/zellij-client/src/stdin_handler.rs:84-100`
- zellij parser 调用: `~/repo/zellij/zellij-client/src/stdin_handler.rs:181`(termwiz `InputParser::parse`,`maybe_more` 用法)
- zellij escape-time: `~/repo/zellij/zellij-client/src/stdin_handler.rs:104`(50ms `recv_timeout`)
- zellij paste 处理: `~/repo/zellij/zellij-client/src/input_handler.rs:178-203`
- zellij raw bytes + semantic key 双轨: `~/repo/zellij/zellij-server/src/route.rs:2232-2251`,`~/repo/zellij/zellij-server/src/tab/mod.rs:3004-3052`
- tmux 经典实现(可对照): `tty-keys.c` 的 ESC 序列前缀树 + `escape-time` 选项
