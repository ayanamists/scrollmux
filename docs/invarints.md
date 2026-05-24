# ScrollMux 不变量

这份文档记录当前设计必须守住的性质，尤其面向下一步要健壮化的两块：

- 渲染路径：`pane vt100 stream -> pane cells -> host terminal drawing`
- 输入路径：`host terminal event -> mux action 或 PTY input bytes`

这些不变量优先级高于具体实现。以后无论继续手写 `crossterm` renderer，还是迁到 `ratatui` / `tui-term`，都应该保持这些性质。

## 术语

- 宿主终端：运行 `scrollmux` 的真实 terminal emulator。
- pane：ScrollMux 内部的一个固定宽度 PTY 会话。
- PTY 字节流：子进程输出给 pane 的原始 bytes，包含 ANSI/VT escape sequence。
- cell：`vt100::Parser` 解析后得到的终端格子，包含字符、颜色、样式、宽字符信息。
- 虚拟工作区：所有 pane 横向拼接后的无限或半无限坐标空间。
- viewport：宿主终端窗口在虚拟工作区里的可见区间。
- mux action：ScrollMux 自己消费的控制命令，例如切 pane、移动 viewport、新建 pane。
- PTY input bytes：转发给 focused pane 子进程 stdin 的 bytes。

## 全局不变量

### Pane 宽度独立于宿主终端宽度

pane 的 `width` 是逻辑终端宽度，不等于宿主终端的 `screen_width`。

必须满足：

- 新增 pane 不会 resize 任何已有 pane。
- viewport 水平移动不会 resize 任何 pane。
- focus 切换不会 resize 任何 pane。
- 宿主终端 resize 默认只改变 pane 高度，不改变 pane 宽度。
- 只有显式的 pane width resize action 可以改变 pane 宽度。

### Focus 和 viewport 是两个概念

`focused` 决定哪个 pane 接收输入。`viewport_x` 决定宿主终端显示虚拟工作区的哪一段。

必须满足：

- 手动滚动 viewport 不应该改变 focus。
- focus 切换可以为了可用性移动 viewport，使 focused pane 可见。
- 一个 pane 可以 focused 但只有部分可见。
- 一个 pane 可以可见但不 focused。

### 不可直接 replay 子进程原始输出

子进程输出的 PTY bytes 坐标系属于 pane 自己，不能直接写到宿主终端。

正确路径是：

```text
PTY output bytes
  -> vt100::Parser
  -> pane-local cell grid
  -> viewport clipping
  -> host terminal drawing
```

任何实现都不能把 child process 的 raw bytes 直接 splice 到宿主终端输出里。否则光标移动、清屏、换色等 escape sequence 会逃逸出 pane 的坐标系。

## 几何与裁剪不变量

### 坐标空间必须显式区分

至少有三种 x 坐标：

- virtual x：虚拟工作区坐标，类型应能容纳所有 pane 宽度之和。
- pane-local x：单个 pane 内部坐标，范围是 `[0, pane.width)`。
- host x：宿主终端屏幕坐标，范围是 `[0, screen_width)`。

实现中不能把这三种坐标隐式混用。重构时推荐使用更明确的类型或结构体表达。

### 区间统一使用半开语义

所有横向区间应统一理解为 `[start, end)`。

例如：

```text
pane range:     [pane_start, pane_end)
viewport range: [viewport_x, viewport_x + screen_width)
visible range:  intersection(pane range, viewport range)
```

这样可以避免边界列被重复绘制或漏绘。

### `visible_panes()` 的返回 contract

对每个返回的 visible pane：

- `idx < panes.len()`
- `src_left < src_right`
- `src_right <= panes[idx].width`
- `dst_left < screen_width`
- `dst_left + (src_right - src_left) <= screen_width`
- `src_right - src_left` 等于 pane 区间和 viewport 区间的交集宽度

对整个返回列表：

- 按虚拟工作区从左到右排序。
- 不包含完全在 viewport 外的 pane。
- host 上的目标区间不重叠。
- 不产生超出 `screen_width` 的目标区间。
- 空 pane 列表返回空列表，不 panic。

### 边界值必须稳定

这些情况应该有明确行为并被测试覆盖：

- `panes` 为空。
- `screen_width == 0` 或非常小。
- `screen_height <= STATUS_BAR_ROWS`。
- `viewport_x == 0`。
- `viewport_x` 正好落在 pane 边界上。
- viewport 正好结束在 pane 边界上。
- 单个 pane 比 viewport 窄。
- 单个 pane 比 viewport 宽。
- 总虚拟宽度小于 viewport 宽度。
- `viewport_x` 大于最大可滚动位置时要 clamp。

## 渲染不变量

### 渲染源只能是 cell model

渲染器的输入应该是 `vt100::Screen` 或等价的 cell model，而不是 PTY raw output。

每个被绘制的 host cell 来自一个明确的源：

```text
host(x, y) = pane[idx].screen.cell(src_x, y)
```

其中 `src_x = visible.src_left + (x - visible.dst_left)`。

### 只绘制 viewport 内的 pane 内容

渲染器不能写出 pane 内容区之外：

- pane 内容只能写到 host row `[0, pane_height)`。
- 状态栏只能写到状态栏 row。
- 不可见 pane 不能被绘制。
- 可见 pane 只能绘制其 `src_left..src_right` 切片。

### 样式是 cell 内容的一部分

cell 的可见语义包括：

- 字符内容
- 前景色
- 背景色
- bold
- italic
- underline
- inverse
- 宽字符 continuation 状态

复制 cell 时不能只复制字符。目标 host cell 的最终显示样式必须等价于源 cell。

### 样式状态必须可恢复

终端样式是状态机。设置颜色或属性后，后续输出会继承这个状态。

渲染器必须保证：

- 当源 cell 样式改变时，host terminal 样式也改变。
- 当某个属性从 on 变 off 时，必须 reset 或发出等价的 off 操作。
- 每行或每个 pane 绘制结束后不能让样式泄漏到状态栏或下一个 pane。
- 整个 render 结束后宿主终端处于可预期状态。

### 宽字符不能破坏坐标

对于 CJK、emoji、组合字符等情况：

- leading cell 应该负责绘制可见字符。
- continuation cell 不应该额外输出一个字符。
- 裁剪从宽字符中间开始时，不能让后半个字符污染 host cell。
- 裁剪在宽字符中间结束时，不能让字符越界写到 viewport 外。

当前实现只做了基础处理：`cell.contents().is_empty()` 且 `is_wide_continuation()` 时跳过。后续需要专门测试宽字符边界裁剪。

### Cursor 只属于 focused pane

宿主终端光标只能表示 focused pane 的 cursor。

必须满足：

- 非 focused pane 的 cursor 不显示。
- focused pane 不可见时，不显示 cursor。
- focused pane cursor 在当前 viewport 外时，不显示 cursor。
- `vt100::Screen::hide_cursor()` 为 true 时，不显示 cursor。
- 显示 cursor 时，host 坐标必须经过同一套 viewport clipping 转换。

## PTY 与 terminal model 不变量

### PTY size 和 parser size 必须同步

每个 pane 有两个相关尺寸：

- PTY 尺寸：子进程看到的 terminal size。
- parser 尺寸：`vt100::Parser` 维护的 screen size。

必须满足：

- pane 创建时两者一致。
- 显式改变 pane 宽度时，两者都更新。
- 宿主终端 resize 改变 pane 高度时，两者都更新。
- viewport 水平移动时，两者都不变。

### 不可见 pane 仍然运行

viewport 只影响显示，不影响生命周期。

必须满足：

- 不可见 pane 的 PTY 继续读写。
- 不可见 pane 的 `vt100::Parser` 继续更新。
- 不可见 pane 的 dirty 状态可以被记录。
- 重新滚动回来时，应该看到该 pane 的最新 screen state。

## 输入不变量

### 每个 key event 只能被分类一次

一个宿主终端 key event 只能落入以下两类之一：

- ScrollMux 自己消费的 mux action。
- 转发给 focused pane 的 PTY input bytes。

不能同时既触发 mux action 又转发给 PTY。

### Mux 快捷键不能泄漏给 PTY

ScrollMux 保留的快捷键，例如 `Alt-h`、`Alt-l`、`Alt-n`、`Alt-w`，必须只影响 mux 状态。

它们不能被编码成 bytes 写入 focused pane，否则子进程会收到用户没有意图发送的 escape sequence。

### 非 mux 输入应尽量保持终端语义

对于未被 ScrollMux 消费的输入，应尽量表现得像用户直接在普通终端中输入：

- 普通字符转 UTF-8 bytes。
- Enter 转 `\r`。
- Tab 转 `\t`。
- Backspace 使用当前项目约定的 `0x7f`。
- 方向键和功能键转常见 xterm escape sequence。
- Ctrl-letter 转控制字符。
- 未保留的 Alt 组合键使用 ESC-prefix。

这是兼容 shell、readline、vim、编辑器和 TUI 子程序的基础。

### 只处理 press/repeat，不处理 release

`KeyEventKind::Release` 不能触发 mux action，也不能转发给 PTY。

否则一次按键可能产生重复动作或重复输入。

### Resize event 不是输入

宿主终端 resize 必须更新 workspace 和 pane 高度，但不能转发给 PTY stdin。

PTY resize 应通过 PTY API 传递 terminal size，而不是通过输入字节传递。

### 输入目标由处理时的 focus 决定

普通输入只写入当前 `focused` pane。

必须满足：

- focus 切换 action 完成后，后续输入进入新的 focused pane。
- viewport 滚动不改变输入目标。
- 如果 focused pane 被关闭，focus 必须被调整到一个有效 pane，或在无 pane 时停止输入转发。

### Paste 必须有明确策略

当前实现启用了 bracketed paste，但 `App::handle_event()` 没有处理 paste event。这是一个已知缺口。

后续需要明确：

- paste 内容是否总是转发给 focused PTY。
- 是否要包裹 bracketed paste sequence。
- paste 中的换行如何编码。
- paste 是否允许触发 mux 快捷键。默认应不允许，paste 应作为数据转发。

## 重构边界

可以替换的实现细节：

- 用 `ratatui` 管 host terminal buffer 和 diff。
- 用 custom widget 渲染 `vt100::Screen`。
- 用更强类型的 geometry 结构替代当前 `VisiblePane` 计算。
- 用 property tests 替代一部分手写 case tests。

不能改变的语义：

- pane 是固定宽度 PTY-backed terminal。
- workspace 是横向 strip。
- viewport 是宿主终端看到的横向切片。
- input focus 和 viewport 位置解耦。
- mux 快捷键先于 PTY 输入转发被消费。
- 子进程原始输出不能直接 replay 到宿主终端。

## 建议测试清单

### Geometry property tests

对随机 pane widths、screen width、viewport_x 验证：

- `visible_panes()` 不返回越界 index。
- 每个 visible pane 的 source range 在 pane width 内。
- 每个 visible pane 的 destination range 在 screen width 内。
- destination ranges 单调递增且不重叠。
- 返回的总目标宽度不超过 `screen_width`。
- 每个返回项都确实与 viewport 相交。
- 每个与 viewport 相交的 pane 都被返回。

### Render golden tests

构造 fake pane screen，验证：

- 单 pane 完全可见。
- pane 左侧被裁剪。
- pane 右侧被裁剪。
- viewport 横跨多个 pane。
- 状态栏不会继承 pane 样式。
- 光标只在 focused pane 可见时出现。
- 彩色、bold、underline、inverse cell 的输出等价。
- 宽字符在裁剪边界处不越界。

### Input table tests

对 `input::map()` 建表验证：

- 每个 mux 快捷键返回正确 action。
- mux 快捷键不返回 `Action::Input`。
- 普通字符、Enter、Tab、Backspace、方向键、F keys 编码符合预期。
- Ctrl-letter 编码符合预期。
- 未保留 Alt 字符编码为 ESC-prefix。
- release event 返回 `None`。
- repeat event 行为和 press 一致。

### Integration smoke tests

后续如果引入自动化 PTY 测试，至少覆盖：

- 在 pane 中运行 shell，输入命令，能看到输出。
- 新建 pane 后旧 pane 宽度不变。
- 横向滚动不会改变任何 pane 的 PTY size。
- 宿主 resize 后 pane 宽度不变，高度变化。
- focused pane 接收输入，非 focused pane 不接收输入。
