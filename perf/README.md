# ScrollMux Perf Suite

这个目录放 ScrollMux 的 terminal-output perf harness。它关注的是当前最敏感的路径：

```text
PTY output bytes -> vt100 cells -> host terminal drawing
host key events  -> mux action / PTY input bytes
```

结果默认写到 `target/perf/<timestamp>/`，避免把 raw terminal output 提交进源码树。

## 运行

可以直接在本机运行，也可以进入 Nix dev shell 固定依赖。本机运行需要：

- `bash`
- `cargo`
- `script`
- `time`（GNU `time` 或 macOS/BSD `/usr/bin/time` 都可以）
- `perl`、`awk`、`wc`
- `nvim`（仅 `nvim-scroll` 场景需要）

使用 Nix：

```bash
nix develop
```

跑全部场景：

```bash
perf/run.sh
```

只跑一个场景：

```bash
perf/run.sh --scenario nvim-scroll
```

列出场景：

```bash
perf/run.sh --list
```

额外收集 syscall summary：

```bash
perf/run.sh --strace
```

## 当前场景

- `plain-output`：大量普通文本输出，衡量静态输出经 ScrollMux 后的放大。
- `ansi-burst`：持续输出 ANSI 控制序列，模拟全屏 TUI/动画式程序。
- `nvim-scroll`：打开 `nvim --clean` 大文件，连续发送 `j`，衡量小更新被 ScrollMux 放大的程度。

每个场景都会跑两次：

- `direct`：目标程序直接跑在伪终端里。
- `scrollmux`：同一个目标程序作为首个 ScrollMux pane 启动。

## 输出

核心文件：

- `summary.md`：人读摘要，包含 terminal bytes 放大倍数。
- `summary.tsv`：机器可读指标。
- `<scenario>/direct.out`：direct raw terminal output。
- `<scenario>/scrollmux.out`：scrollmux raw terminal output。
- `<scenario>/*.time`：`time` 结果；Linux/GNU time 和 macOS/BSD time 格式都会被解析。
- `<scenario>/*.strace`：启用 `--strace` 时生成。

目前采集的指标：

- 输出字节数。
- `Clear(All)` / `Clear(CurrentLine)` 次数。
- cursor move escape sequence 次数。
- SGR/style escape sequence 次数。
- alternate screen enter/leave 次数。
- user/system time。
- max RSS。

## 场景约定

场景是 `perf/scenarios/*.sh` 下的 Bash 文件，提供以下接口：

```bash
SCENARIO_NAME="name"
SCENARIO_ROWS=40
SCENARIO_COLS=120

scenario_prepare() { ... }
scenario_command() { ... }
scenario_drive_direct() { ... }
scenario_drive_scrollmux() { ... }
```

`scenario_prepare DIR` 应在 `DIR` 下生成 fixture 和可执行 command wrapper。

`scenario_command DIR` 输出 wrapper 路径。这个 wrapper 会被 direct 和 scrollmux 两种模式复用；scrollmux 模式下它会作为 `SHELL` 启动。

`scenario_drive_*` 向伪终端 stdin 写入输入序列。ScrollMux 模式通常需要最后发送 `Alt-q`，即：

```bash
printf '\033q'
```
