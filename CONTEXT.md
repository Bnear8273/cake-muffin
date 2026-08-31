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
signals, process groups, and running shell code in a sub-shell. Split out of
`Platform` so a target without Unix primitives (e.g. no `fork`) can still
implement the shell.

Boundary: takes `spawn`/`wait`/`kill`/`run_in_child`, all signal handling,
`set_terminal_foreground`/`current_process_group`, `geteuid`/`getegid`/
`parent_pid`, `fd_path`, and types `ProcessHandle`/`ProcessGroupId`/
`WaitStatus`/`SpawnConfig`/`WaitOptions`.
_Avoid_: exec layer, subprocess manager

**run_in_child**:
The current `Platform` hook that runs a closure in a child process. On Unix
this is implemented with `fork()` + a raw-pointer reborrow of the live
`Executor`. It is the single most Unix-coupled surface in the design.
_Avoid_: fork hook, child runner

**fork**:
The Unix primitive the current backend leans on to snapshot heap memory and
continue running shell code in-place inside a child. Windows has no
equivalent, which is why `run_in_child` must be redesigned.

**Backend**:
A concrete implementation of `Platform` (and, in future, `ProcessModel`) for
one OS. Existing: Unix, mock. Planned: Windows.

**fd (file descriptor)**:
An integer identifying an open resource — a file, pipe, or terminal — that
the shell reads from or writes to. `0`/`1`/`2` are stdin/stdout/stderr. The
`Fd` type hides the platform's native flavour: `i32` on Unix, `usize`
elsewhere.
_Avoid_: handle, stream id
