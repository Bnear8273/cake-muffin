# cake-shell

A bash-compatible shell written in Rust. The domain centers on two ideas:
shell *semantics* (pure logic) vs. how the OS actually runs things
(platform-dependent).

## Language

**Platform**:
The lowest-level OS abstraction the shell talks to: files, descriptors, I/O,
terminal modes, CWD, time. Owns no process semantics.
_Avoid_: backend, driver, syscall layer

**ProcessModel**:
The abstraction for *running things*: spawning children, waiting, killing,
signals, process groups, and running shell code in a sub-shell. Inherits
`Platform` as a supertrait so a single `&dyn ProcessModel` reference gives
access to both OS primitives and process semantics. A target without Unix
primitives (e.g. no `fork`) implements `ProcessModel` with default stubs
for unsupported operations (`Err(Unsupported)`).

Boundary: takes `spawn`/`wait`/`kill`/`fork_and_run`, all signal handling,
`set_foreground_process_group`/`current_process_group`,
`effective_user_id`/`effective_group_id`/`parent_pid`/`fd_to_path`,
and types `ProcessHandle`/`ProcessGroupId`/`WaitStatus`/`SpawnConfig`/
`WaitOptions`.
_Avoid_: exec layer, subprocess manager

**fork_and_run**:
The `ProcessModel` method that runs a closure in a child process. On Unix
the backend uses `fork()` to create the child; callers snapshot shell state
(env, positionals, functions, aliases, flags) and build a fresh `Executor`
inside the child (clone-and-build, 5 call sites across `executor.rs` and
`expand.rs`; the old raw-pointer reborrow is gone since 2026-09-11). It
remains the most Unix-coupled surface: a backend without `fork` must create
the child differently (e.g. spawn a fresh `cake -c <script>` process with
explicitly passed state).
_Avoid_: run_in_child, fork hook, child runner

**fork**:
The Unix primitive the backend's `fork_and_run` uses to create a child
process with inherited OS-level state (fds, CWD, signals). Callers no longer
depend on `fork` snapshotting heap memory (clone-and-build since 2026-09-11);
what remains is the need to conjure a child that inherits OS state, which
Windows must emulate with `CreateProcess` + explicit state passing.

**Backend**:
A concrete implementation of `ProcessModel` (which includes `Platform`) for
one OS. Existing: Unix, mock. Planned: Windows.

**fd (file descriptor)**:
An integer identifying an open resource — a file, pipe, or terminal — that
the shell reads from or writes to. `0`/`1`/`2` are stdin/stdout/stderr. The
`Fd` type hides the platform's native flavour: `i32` on Unix, `usize`
elsewhere.
_Avoid_: handle, stream id

## Architecture Review (2026-09-03)

### Deepening Candidates

**C1: Split Platform + ProcessModel (Strong)**
Platform trait 混合了两个职责：Platform（文件、fd、终端、CWD、时间）和 ProcessModel（spawn、wait、kill、信号、run_in_child）。Mock 适配器已部分展示这种分离——支持 Platform 方法，但对 ProcessModel 方法返回 Unsupported。拆分后创建两个深度模块，ProcessModel 成为未来 Windows 后端的自然接缝。
- 文件: `crates/cake-platform/src/lib.rs:315-421`
- **Status (2026-09-11): done.** `ProcessModel: Platform`（超级 trait）；`get()` 返回 `&'static dyn ProcessModel`；Unix/Mock 均拆分为双 impl；15 个方法 + `Termios`→`TerminalState` 已重命名；12 个方法有默认实现。

**C2: Thread &dyn Platform through Executor (Strong)**
cake-exec 中 92 个调用点通过全局 `cake_platform::get()` 访问平台。MockPlatform 已存在但被孤立——无测试依赖。将 `&dyn Platform` 穿透到 Executor 及其协作者中，将全局接缝转变为真实适配器接缝，为整个 cake-exec 解锁 mock 测试。
- 文件: `crates/cake-exec/src/executor.rs`, `builtins.rs`, `cond.rs`, `redirect.rs`, `expand.rs`, `glob.rs`, `path.rs`
- **Status (2026-09-11): done.** cake-exec 内 `get()` 清零（已验证）；`Executor<'a>` + `ExpandCtx` 持有 `&dyn ProcessModel`；Mock 接入 dev-dependencies，已有 15+ 真实 mock 测试（文件测试、`cd`/`pwd`、glob、`[[ ]]`）。

**C3: Unify test operator evaluation (Worth exploring)**
`[[ ... ]]` (cond.rs) 和 `[ ... ]`/`test` (builtins.rs) 独立实现了相同的文件测试和比较运算符，`parse_num` 被字面复制。提取共享测试求值模块，局部性：添加新运算符只需修改一处。
- 文件: `crates/cake-exec/src/cond.rs:120-299`, `crates/cake-exec/src/builtins.rs:451-680`
- **Status (2026-09-11): done.** 提取 `test_ops` 模块（`parse_num`/`eval_unary_file_test`/`eval_binary_file_test`），两处共用。

**C4: Extract sub-shell Executor construction (Worth exploring → 现在最优先)**
三个独立位置手动复制 7-8 个字段构造子 shell Executor。提取 `Executor::fork_shell_state()` 集中此逻辑，避免并行结构漂移。
- 文件: `crates/cake-exec/src/expand.rs:412-439`, `expand.rs:491-498`, `executor.rs:1380-1405`
- **Status (2026-09-11): open, scope grew — 现 5 处。** Phase 3 把 3 处裸指针改成 clone-and-build，手动快照点从 3 涨到 5：`executor.rs`（background `&`、subshell `()`、coproc）+ `expand.rs`（进程替换、命令替换）。漂移风险升高，应提取统一快照构造。

**C5: Introduce RAII fd wrapper (Speculative)**
无 RAII fd 包装器，手动 close 散布各处。`apply_fds_in_parent` 无 panic 安全性。引入 `GuardedFd` 将 fd 生命周期集中在一处。
- 文件: `crates/cake-exec/src/redirect.rs:16-24`, `executor.rs:1483-1525`
- **Status (2026-09-11): open.** 未动；现为唯一剩余候选项。

**C6: Delete dead crates (Worth exploring)**
`cake-builtin` 为空占位符，`cake-proc` 定义了从未使用的 `Process`/`Job` 类型。删除测试：不集中复杂性，只移除噪音。
- 文件: `crates/cake-builtin/`, `crates/cake-proc/`
- **Status (2026-09-11): done, 超出原计划。** `cake-builtin` 整 crate 删除；`cake-proc` 不止删死类型，整个 crate 合并进 `cake-exec::proc_status` 后删除（工作区 15→13 crate）。

### Top Recommendation
C1 + C2 深度关联，应一起处理。拆分创建 ProcessModel 接缝；穿透 &dyn Platform 将全局单例转变为适配器接缝。两者结合为 cake-exec（最复杂且测试最少的部分）解锁 mock 测试。
- **Status (2026-09-11): fulfilled.** C1+C2 按预期兑现，mock 测试已解锁（15+ mock 测试）。新推荐：**C4**——5 处子 shell 快照需统一构造，防止漂移。

## Unix Coupling Analysis (2026-09-03)

### Coupling Map

所有 `nix`/`libc` 调用**仅存在于一个文件**：`crates/cake-platform-unix/src/lib.rs`（2026-09-11 复核仍成立；`cake/src/main.rs` 的一处命中是 `platform_unix::` 子串误报）。
`cake-exec`（shell 逻辑核心）零直接 Unix 导入——现经 `Executor.platform: &dyn ProcessModel` 间接调用（`get()` 已清零）。

| 类别 | 数量 | 位置 |
|------|------|------|
| `nix::` 出现 | 70 处 | 全部在 `cake-platform-unix`（`rg -o` 口径；原 46 为 call 口径） |
| `libc::` 出现 | 58 处 | 全部在 `cake-platform-unix`（`rg -o` 口径；原 45 为 call 口径） |
| `unsafe` | 35 处 | 全部在 `cake-platform-unix`（`cake-platform` 仅 `forbid` 属性行） |
| `fork()` 调用 | 2 处 | `spawn()` 和 `fork_and_run()`（已改名，原 `run_in_child`） |

### Three True Unix Dependencies

**1. `fork_and_run` 的 fork 语义（调用方 unsafe 已消除，2026-09-11）**
原 `executor.rs:564-566` 模式（`&mut self`→`*mut Executor` 裸指针，子进程解引用）已全部改为 clone-and-build，cake-exec 现零 `unsafe`。剩余耦合在后端实现内部：`fork()` 凭空变出带继承状态（fds、CWD、信号处置）的子进程。Windows 的 `CreateProcess` 启动全新进程，需显式传递状态——仍是最大的移植障碍，但难度已从"堆快照不可移植"降为"子进程创建方式不同"。
- 调用点（均为 clone-and-build）: `executor.rs`（背景任务 `&`、子 shell `()`、coproc）+ `expand.rs`（进程替换 `<()`/`>()`、命令替换 `$()`），共 5 处

**2. 信号数字硬编码（已修复，2026-09-11）**
原 `parse_trigger` 中 `HUP=1, KILL=9, PIPE=13` 等 Linux 数字已全部替换为 12 个新 `Signal` 变体（Hangup/Kill/Pipe/Alarm/Stop/Ill/Abort/Bus/FloatingPoint/Segmentation/Ttin/Ttou），数字映射收归各后端。纠正原判断"不会导致正确性问题"：macOS 下 SIGBUS（7 vs 10）、SIGSTOP（19 vs 17）编号不同，此前 `trap ... BUS/STOP` 会装错 handler。

**3. 二进制 crate 直接使用 `std`（绕过 Platform）**
`repl.rs` 中有 3 处 `std::fs::read_dir()` 应该用 `Platform::read_dir()`，1 处 `std::io::IsTerminal` 应该用 `Platform::is_terminal_fd()`。
- **Status (2026-09-11): partial.** `read_dir` 3 处已改（含补全路径拼接辅助 `join_path`）；`IsTerminal` 仍残留（`cake/src/repl.rs:214`），待修。

### Platform Coupling Table

| 分类 | Crate 数量 | 说明 |
|------|-----------|------|
| **已平台无关** | 12 | `cake-syntax`, `cake-env`, `cake-blacklist`, `cake-complete`, `cake-reader`, `cake-editor`, `cake-highlight`, `cake-prompt`, `cake-exec/arith`, `cake-exec/resolve` 等 |
| **已穿透引用** | 7 模块 | `executor`, `builtins`, `cond`, `expand`, `redirect`, `glob`, `path`——经 `Executor.platform` 访问，mock 测试已落地 |
| **故意 Unix 耦合** | 1 | `cake-platform-unix`——后端实现，应保持 |
| **mixed** | 1 | `cake` 二进制 crate——作为 driver 用 `get()` 初始化并传入 `Executor::new`（by design）；补全 `read_dir` 已收敛到 Platform，`IsTerminal` 仍直连 `std` |

### Hardcoded Unix Assumptions in Strings

| 位置 | 假设 | 状态 |
|------|------|------|
| `cake-env/env_var.rs:51` | `PATH_DELIMITER: char = ':'` | ✅ 已修复（`cfg(windows)`→`;`） |
| `cake-exec/glob.rs` pattern 解析 | `.split('/')` | ✅ 已修复（`is_path_separator()` 驱动拆分） |
| `cake-exec/glob.rs` 根目录 | `"/"` 字面量 | ✅ 已修复（`path_separator()` 构造 `root`） |

### Portability Conclusion

架构设计是正确的——Unix 耦合被有意地隔离在 `cake-platform-unix` 中。真正的移植障碍曾是 `run_in_child` 的 fork 语义；2026-09-11 后调用方已全部改为 clone-and-build，"复制堆内存"的要求消失。剩余障碍收敛为一点：后端必须能**凭空变出带继承 OS 状态（fds、CWD、信号处置）的子进程**——Unix 用 `fork()` 免费获得，Windows 需 `CreateProcess` + 显式状态传递。命令替换、进程替换、背景任务、子 shell、coproc 现在使用同一模式，移植面已统一。

## Unix Decoupling Plan (2026-09-04)

**Status (2026-09-11): all 4 phases done.** 偏差见文末新节；trait 方法计数 29 必须 + 12 可选依然有效（无增减）。

### Goal

将 cake-shell 从"隐式 Unix 耦合"转变为"显式平台抽象"，解锁 mock 测试并为自定义 OS 后端铺路。

### Design Principles

1. **一个引用访问一切**：`ProcessModel : Platform`（超级 trait），`&dyn ProcessModel` 自动获得 Platform 方法
2. **默认值只用于"合法退化"**：`Err(Unsupported)` 表示"OS 不支持此功能"，不用于猜测平台行为
3. **方法名描述语义而非实现**：不用 Unix 行话（`dup`、`stat`、`termios`），用描述性动词
4. **自定义 OS 最小实现 ~30 个方法**（现有 35 个全部必须实现）

### Trait Design

```
ProcessModel : Platform
```

- **Platform**（24 方法）：fd 操作、终端、文件系统、CWD、时间
- **ProcessModel**（17 方法）：进程管理、信号、`fork_and_run`、进程信息

### Method Naming

**Platform trait — 需要改名的方法：**

| 当前名 | 新名 | 理由 |
|--------|------|------|
| `pipe(cloexec)` | `create_pipe_pair(cloexec)` | 与 `std::io::pipe` 区分 |
| `dup(fd)` | `duplicate_fd(fd)` | "dup" 是 Unix 行话 |
| `dup2(oldfd, newfd)` | `duplicate_fd_to(oldfd, newfd)` | "dup2" 是 Unix 行话 |
| `get_termios(fd)` | `read_terminal_state(fd)` | "termios" 是 POSIX 术语 |
| `set_termios(fd, t)` | `write_terminal_state(fd, state)` | 同上 |
| `stat(path)` | `file_info(path)` | "stat" 是 Unix 行话 |
| `fd_path(fd)` | `fd_to_path(fd)` | 更明确 |
| `Termios` struct | `TerminalState` | 不泄露 POSIX 术语 |

**ProcessModel trait — 需要改名的方法：**

| 当前名 | 新名 | 理由 |
|--------|------|------|
| `run_in_child(f)` | `fork_and_run(f)` | 描述机制 |
| `set_terminal_foreground(pgid)` | `set_foreground_process_group(pgid)` | 更明确 |
| `signal_number(sig)` | `signal_to_number(sig)` | 明确方向 |
| `drain_received_signals()` | `receive_pending_signals()` | "drain" 不常见 |
| `geteuid()` | `effective_user_id()` | Unix 缩写 |
| `getegid()` | `effective_group_id()` | Unix 缩写 |
| `fd_path(fd)` | `fd_to_path(fd)` | 同 Platform |

**不需要改名的方法（26 个）：** `open_file`、`close`、`write`、`read`、`terminal_size`、`null_device`、`path_separator`、`is_path_separator`、`is_executable`、`is_terminal_fd`、`read_dir`、`xdg_dir`、`current_dir`、`set_current_dir`、`time_seconds`、`time_nanos`、`local_time_hms`、`parent_pid`、`spawn`、`wait`、`kill`、`current_process_group`、`install_signal_handler`、`signal_from_number`、`signal_name`、`install_trap_handler`、`block_signals`、`unblock_signals`

### Optional Methods (Defaults)

**Platform — 3 个可选：**

| 方法 | 默认行为 | 理由 |
|------|----------|------|
| `read_terminal_state()` | `Err(Unsupported)` | 非交互式 shell 不需要 |
| `write_terminal_state()` | `Err(Unsupported)` | 同上 |
| `terminal_size()` | `Err(Unsupported)` | 同上 |

**ProcessModel — 9 个可选：**

| 方法 | 默认行为 | 理由 |
|------|----------|------|
| `set_foreground_process_group()` | `Err(Unsupported)` | 无 job control 的 OS |
| `install_signal_handler()` | `Err(Unsupported)` | 无信号的 OS |
| `install_trap_handler()` | `Err(Unsupported)` | 同上 |
| `block_signals()` | `Err(Unsupported)` | 同上 |
| `unblock_signals()` | `Err(Unsupported)` | 同上 |
| `receive_pending_signals()` | `Vec::new()` | 无信号时返回空 |
| `fork_and_run()` | `Err(Unsupported)` | 无 fork 的 OS |
| `effective_user_id()` | `0` | 无用户概念的 OS 默认 root |
| `effective_group_id()` | `0` | 同上 |

### Implementation Count

| Trait | 必须实现 | 可选 | 合计 |
|-------|----------|------|------|
| Platform | 21 | 3 | 24 |
| ProcessModel | 8 | 9 | 17 |
| **总计** | **29** | **12** | **41** |

对比现在 35 个全部必须实现，自定义 OS 的负担减少 **17%**。

### Phase 1: 拆分 Platform + ProcessModel

**目标：** 将 Platform trait 拆分为两个 trait，不改变 cake-exec 代码。

**Step 1.1 — 在 cake-platform 中创建 ProcessModel trait**

文件: `crates/cake-platform/src/lib.rs`

从 Platform 移出 Process/Signal/Subprocess/ProcessInfo 方法到新的 `ProcessModel` trait。ProcessModel 继承 Platform（超级 trait）。给 12 个方法加默认实现。

**Step 1.2 — 重命名 15 个方法 + 1 个类型**

文件: `crates/cake-platform/src/lib.rs`, `crates/cake-platform-unix/src/lib.rs`, `crates/cake-platform-mock/src/lib.rs`

按命名表重命名。全局访问器改为返回 `&'static dyn ProcessModel`。

**Step 1.3 — 更新 Unix 后端**

文件: `crates/cake-platform-unix/src/lib.rs`

拆分为 `impl Platform for UnixPlatform` + `impl ProcessModel for UnixPlatform`。

**Step 1.4 — 更新 Mock 后端**

文件: `crates/cake-platform-mock/src/lib.rs`

拆分为 `impl Platform for MockPlatform` + `impl ProcessModel for MockPlatform`。

**Step 1.5 — 更新 cake-exec 导入**

文件: `crates/cake-exec/src/*.rs`

所有 `use cake_platform::Platform` 改为 `use cake_platform::ProcessModel`。

**验证：** `cargo check && cargo test`

### Phase 2: 穿透 `&dyn ProcessModel` 到 Executor

**目标：** 将 92 个 `cake_platform::get()` 调用替换为通过 Executor 传递的引用。

**Step 2.1 — Executor 持有 ProcessModel 引用**

文件: `crates/cake-exec/src/executor.rs`

```rust
pub struct Executor<'a> {
    pub platform: &'a dyn ProcessModel,
    // ... 其余字段不变
}
pub fn new(env: EnvStack, platform: &'a dyn ProcessModel) -> Self
```

**Step 2.2 — 更新 ExpandCtx**

文件: `crates/cake-exec/src/expand.rs`

```rust
pub struct ExpandCtx<'a> {
    pub platform: &'a dyn ProcessModel,
    // ...
}
```

**Step 2.3 — 替换所有 cake_platform::get()**

文件: `executor.rs`(20), `builtins.rs`(28), `cond.rs`(28), `expand.rs`(5), `redirect.rs`(9), `glob.rs`(1), `path.rs`(1)

全部改为 `self.platform` 或从参数传入的 `platform`。

**Step 2.4 — 更新 binary crate**

文件: `cake/src/main.rs`, `repl.rs`, `readline.rs`, `prompt.rs`

```rust
let mut executor = Executor::new(env, cake_platform::get());
```

**验证：** `cargo check && cargo test`

### Phase 3: 改进 fork_and_run 可移植性

**目标：** 消除 executor.rs 中的 `unsafe` 裸指针重借用。

将 `eval_and_or_background`、`CommandKind::Subshell`、`eval_coproc` 从裸指针模式改为状态克隆模式（与 expand.rs 中的命令替换一致）。

**验证：** `cargo check && cargo test`

### Phase 4: 清理

- 删除死代码 crate: `cake-builtin`, `cake-proc`
- 统一测试运算符: 提取 cond.rs + builtins.rs 共享模块

### 依赖顺序

```
Phase 1 (拆分 trait + 重命名)
    ↓
Phase 2 (穿透引用)
    ↓
Phase 3 (改进 fork_and_run)
    ↓
Phase 4 (清理)
```

### Completion Notes (2026-09-11)

Phase 1–4 均按依赖顺序完成，验证均为 `cargo check && cargo test` 全绿。与本计划的偏差（计划文本保留原样）：

- **Phase 1.5 范围扩大**：除导入外，`out()` 等 builtin 输出函数也收 `platform` 参数（18 调用点）；`try_get()`/`with_signals_blocked` 因零调用被删除。
- **计划外：`Signal` +12 变体**——原计划称硬编码数字"不会导致正确性问题"，实际 macOS 下 BUS/STOP 会装错 handler，已修复。
- **计划外：Mock 升级**——`set_file_info`/`set_dir_entries`/write 录制 + 首批 15 个真实 mock 测试（文件测试、`cd`/`pwd`、glob、`[[ ]]`、信号往返）。
- **Phase 4 超出**：`cake-proc` 整 crate 合并进 `cake-exec::proc_status` 后删除（工作区 15→13 crate）；另删除 `ExpandCtx.parent_pid` 快照字段（改 live 读取）。
- **遗留 TODO**：`cake/src/repl.rs:214` 的 `std::io::IsTerminal` 未收敛到 `Platform::is_terminal_fd()`。

## Sessions after 2026-09-04 (2026-09-11 合记)

计划外但已完成的工作，按执行顺序：

1. **Phase 2 收尾**：`out(platform, s)`（`echo`/`printf`/`print_alias` 新增参数，18 调用点）；删除死 API `try_get()`/`with_signals_blocked()`。
2. **可移植性缺口**：glob `/` 硬编码→`is_path_separator()`/`path_separator()` 驱动；`PATH_DELIMITER`→`cfg(windows)`；repl 补全 3 处 `read_dir`→`platform.read_dir()`（+`join_path` 辅助）；`Signal` +12 变体，`parse_trigger` 零硬编码。
3. **Mock 兑现**：`set_file_info`/`set_dir_entries`/write 录制（`take_writes`/`written_to`）；15 个真实 mock 测试（`test` 文件/权限/二元运算符、`cd`/`pwd` 输出断言、glob、`[[ ]]`、`$?` 透传、信号往返）。
4. **P2 清理**：`cake-proc`→`cake-exec::proc_status` 并删 crate；`ExpandCtx.parent_pid` 快照字段删除（`$PPID` 改 live 读取）；mock 首个单测（信号映射表）。

最终状态：`cargo check` 通过 · **238 测试全过** · `clippy --all-targets` 零警告 · cake-exec 零 `unsafe`、零 `cake_platform::get()` · 工作区 15→13 crate。
