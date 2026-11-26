# Implementation Plan: fix the WebView ↔ Backend IPC review findings

Branch `desktop-hostapi-e2e` @ `8ab9bdd`. Sources: the IPC review and the review of the first
version of this plan. Every finding of both reviews is a task or a decision below. Nothing is
left as a question.

## Overview

Ten tasks. Each defect gets a test that fails on today's code on an assertion about the defect
(not a compile error and not a hang), then the smallest fix, then the same unchanged test green.
A defect reachable from the real binary is tested on `lunarstorm-debug`. Where only the
structure can be tested (worker threads, Task 4), the plan says so and names the test.

## Decisions

| # | Decision | Rejected alternative |
|---|---|---|
| D1 | `permissions.fs` scopes the `fs` area and the native services. The backend's own `node:fs` gets no folders, as before `426b324`. New field `Program.area_roots` | One shared scope for backend and area: grants existing apps' backends file access without a manifest change |
| D2 | `fs` work runs on worker threads (`lunarstorm-fs`), at most 8 at a time per backend; a 9th gets `EAGAIN`. Results return through `Settler::settle`, like native calls. A hung network mount ties up one worker, never the backend | Synchronous with checks only: bounds local files, not a hung mount. The Runtime's `node:fs/promises`: its scope is the backend's, which D1 separates |
| D3 | `readText`/`writeText` open with `O_NONBLOCK`, `fstat` the open handle and require a regular file (`EINVAL` otherwise). `list` requires a directory (`ENOTDIR`) | `metadata()` then `open()`: racy, and opening a FIFO blocks inside `open()` |
| D4 | Hard links: an existing file whose link count is > 1 is refused for read and write (`EACCES`) | Allowing it: a pre-existing hard link to an outside file passes every path check |
| D5 | A broken or looping symlink anywhere in the path is refused (`EACCES`), even one whose target would lie inside the root | Following `read_link`: more code, and the target can be created later anyway |
| D6 | Limits: `MAX_TEXT` = 16 MiB per `readText` in `fs.rs` (read through `take(MAX_TEXT + 1)`: `E2BIG` if more arrives). `MAX_RESULT` = 16 MiB per `result` envelope in `envelope.rs`, next to `MAX_MESSAGE`, enforced in `Bridge::send` (`E2BIG`, same id). `writeText` is bounded by `MAX_MESSAGE` (1 MiB) inbound | One constant in the host crate: that crate knows no envelopes |
| D7 | `MAX_CALLS` = 64 calls in flight per form; one more gets `EAGAIN`, not forwarded | No bound |
| D8 | A page's `session.start` drops the form's in-flight calls; their late results are discarded | Per-page id prefixes: more protocol for the same effect |
| D9 | Every `AppRuntime` answers every `call`. The `mock` runtime (selectable at build time) answers `EUNAVAIL` | Host-side timeout only: 30 s wait per call |
| D10 | One `pub type Areas = (Fs,)` feeds both `spec()` and `Host::Extensions` | Two hand-kept lists |
| D11 | After `ETIMEDOUT` or `ERELOAD` the operation **may have run**; calls are not cancelled. Documented, not implemented | A cancel envelope: no area needs it yet |
| D12 | `window.__zebraReceive` stays writable (page and shell are one trust domain; the host is the boundary). Calls are not blocked by a modal (they are API use, not input) | Freezing it: breaks nothing today, protects nothing |
| D13 | No new JS files. Area modules stay inline strings in Rust. Test backends and pages are strings in the Rust test files | — |

## Task list

### Phase 1: Security

#### Task 1: Links cannot widen a root (review #1, plan review #5, #6)

**Description:** In `within()` (`platform/desktop/host/src/lib.rs`), a component that
`symlink_metadata` reports as a symlink but that cannot be canonicalised refuses the path (D5).
In `fs.rs`, an existing regular file with `nlink > 1` is refused (D4), using the `fstat` of
Task 3. Comments at both sites name the defect, the decision and the dropped alternative.

**Acceptance criteria:**
- [ ] `a_broken_symlink_inside_a_root_is_refused`: `root/link → <base>/new`. Both the backend's
  `writeFileSync` and a `call` to `fs.writeText('link')` must give `EACCES`, and `<base>/new` must
  not exist. Today `<base>/new` is created (red).
- [ ] `a_hard_link_inside_a_root_is_refused` (unix): `root/hard` hard-linked to `<base>/secret`.
  `fs.readText('hard')` → `EACCES`, `fs.writeText('hard', 'x')` → `EACCES`, secret unchanged.
  Today the read returns the secret (red).

**Verification:** `cargo test -p lunarstorm-host-desktop --test backend`
**Dependencies:** Task 3 (for the hard-link half: it uses the open handle's `fstat`) ·
**Files:** `platform/desktop/host/src/lib.rs`, `src/fs.rs`, `tests/backend.rs` · **Scope:** S

#### Task 2: The backend's `node:fs` no longer inherits `permissions.fs` (D1)

**Description:** `Program` gains `area_roots: Vec<PathBuf>` (the `fs` area's scope). `roots`
stays the backend's `node:fs` scope. The platform passes `permissions.fs` as `area_roots` and an
empty `roots`. `platform/desktop/host/src/main.rs` (the test shell) passes its script directory
as both.

**Acceptance criteria:**
- [ ] New test `the_backend_gets_no_fs_roots_from_permissions_fs` in
  `platform/desktop/app/src/runtime/tests.rs` (real `LunarstormRuntime`). Config `permissions.fs =
  [dir]`; the backend runs `readFileSync(dir/a.txt)` and logs the outcome. It expects `EACCES`;
  today it logs the content (red).
- [ ] `tests/e2e_hostapi.rs` (WebView `fs.readText` under `permissions.fs`) stays green.

**Verification:** `cargo test -p lunarstorm-platform` · `cargo test -p lunarstorm-host-desktop`
**Dependencies:** None · **Files:** `platform/desktop/host/src/lib.rs`, `src/main.rs`,
`tests/backend.rs` (the `Program` literal), `platform/desktop/app/src/runtime/lunarstorm.rs`,
`platform/desktop/app/src/runtime/tests.rs` · **Scope:** M

### Checkpoint: Security
- [ ] `lunarstorm-host-desktop`, `lunarstorm-platform` green, counts read; clippy clean.

### Phase 2: An untrusted page cannot stall or confuse the host

#### Task 3: `fs` touches only regular files and directories (review #5, plan review #1, #3)

**Description:** Implement D3: open with `O_NONBLOCK` (`OpenOptionsExt::custom_flags`), `fstat`
the handle, require `is_file()`, then clear `O_NONBLOCK` and read. `writeText` to an existing path
does the same before truncating. `list` maps `NotADirectory` to `ENOTDIR`.

**Acceptance criteria:**
- [ ] `a_fifo_in_a_root_does_not_block_the_backend` (unix): `mkfifo root/pipe`; call
  `fs.readText('pipe')`, then `fs.list('.')`. Both results must arrive within 10 s:
  `EINVAL` for the read, ok for the list. Today the backend thread blocks and `next()` panics
  (red).
- [ ] The test declares a guard after the backend (so it is dropped first). The guard opens
  `root/pipe` with `O_WRONLY | O_NONBLOCK` and ignores `ENXIO`, so the red run releases a blocked
  reader and the suite ends instead of hanging.

**Verification:** `cargo test -p lunarstorm-host-desktop --test backend`
**Dependencies:** None · **Files:** `platform/desktop/host/src/fs.rs`, `tests/backend.rs` ·
**Scope:** S

#### Task 4: `fs` work runs off the backend thread (review #5: hung network mount; D2)

**Description:**
- The `fs` natives are replaced by one desktop native, `desktopWork(id, function, argsJson)`
  (`lib.rs`), and an `ops`-free area module that calls `work('fs.<fn>', args)`. `work` is a new
  export of the glue (`desktop.js`) that shares the `pending` map of native calls.
- `desktopWork` hands the job to `Workers` (`fs.rs`: an atomic counter, limit 8, one named thread
  per job). The job's result goes back through `Settler::settle`, so retired epochs drop it.
- Coded errors cross as JSON `{code, message}`. The glue's `settle` rejects with an `Error`
  carrying `code` (plain string errors of native calls are unchanged).

**Acceptance criteria:**
- [ ] Unit tests in `fs.rs`:
  - `start` returns while its job is still blocked on a channel;
  - with 8 blocked jobs, the 9th `start` is refused;
  - after release, `start` succeeds again.
  These pin the structure. A hung mount cannot be reproduced on a local disk, so there is no red
  test for it; the FIFO case of the same defect is Task 3's red test.
- [ ] All `fs` tests in `tests/backend.rs` (Tasks 1, 3, 5 and the existing ones) pass unchanged
  through the worker path.
- [ ] `call` while 8 jobs are blocked → `EAGAIN`; covered by the unit test of the limit plus a
  `backend.rs` assertion that the code reaches the `result`.

**Verification:** `cargo test -p lunarstorm-host-desktop` · `cargo test -p lunarstorm-platform --test e2e_hostapi`
**Dependencies:** Tasks 1, 3 (their checks move into the job unchanged) · **Files:**
`platform/desktop/host/src/lib.rs`, `src/fs.rs`, `js/desktop.js` · **Scope:** M

#### Task 5: Results are bounded (review #3, plan review #4, #8, #13)

**Description:** Implement D6. `readText` reads through `take(MAX_TEXT + 1)`: more than
`MAX_TEXT` bytes → `E2BIG`. `Bridge::send` (`platform/desktop/app/src/runtime/lunarstorm.rs`)
replaces a `result` envelope longer than `MAX_RESULT` with a failed `result` (`E2BIG`, same id
and window).

**Acceptance criteria:**
- [ ] `a_file_over_the_text_limit_is_refused` (`backend.rs`): a file of `MAX_TEXT + 1` bytes →
  `E2BIG`. Today it returns the content (red).
- [ ] `a_result_over_the_envelope_limit_becomes_e2big` (`runtime/tests.rs`, real runtime):
  `permissions.hostapi = ["fs"]`, a 3 MiB file of `\u0001`. It passes `MAX_TEXT`, but escaped
  as JSON it is ~18 MiB. The `result` the ui gets must be `E2BIG`; today it is the ~18 MiB value
  (red).

**Verification:** `cargo test -p lunarstorm-host-desktop` · `cargo test -p lunarstorm-platform`
**Dependencies:** Task 4 (same read path) · **Files:** `platform/desktop/host/src/fs.rs`,
`tests/backend.rs`, `platform/desktop/app/src/envelope.rs`, `src/runtime/lunarstorm.rs`,
`src/runtime/tests.rs` · **Scope:** M

#### Task 6: A new page does not inherit the old page's calls (review #2, plan review #2, #9)

**Description:** Implement D8 in `on_ipc`'s `Inbound::Hello` (`platform/desktop/app/src/ui/mod.rs`).

**Acceptance criteria:**
- [ ] New e2e test `a_reloaded_page_does_not_inherit_the_old_pages_calls` with its own fixture
  and temp directory. The existing no-backend e2e stays unchanged.
  - Fixture: the backend string has `Busy.onClick` = a 3 s loop (budget 5 s), and the form has a
    `Busy` button.
  - Page load 1 (no hash): `zebra.emit('Busy', 'onClick')`, `fs.readText('in.txt')` (queued
    behind the busy backend), then `location.hash = 'two'; location.reload()`.
  - Page load 2 (`#two`): `fs.writeText('reload.txt', 'ok')`, and on failure writes the code
    instead.
  - Expected `reload.txt == "ok"`; today it is `"EREFUSED"` (red).

**Verification:** `cargo test -p lunarstorm-platform --test e2e_hostapi`
**Dependencies:** None · **Files:** `platform/desktop/app/src/ui/mod.rs`,
`platform/desktop/app/tests/e2e_hostapi.rs` · **Scope:** S

#### Task 7: At most `MAX_CALLS` in flight per form (review #4)

**Description:** Implement D7 in `on_ipc`'s `Inbound::Call`.

**Acceptance criteria:**
- [ ] New e2e test `a_page_cannot_queue_more_than_max_calls`, using Task 6's fixture. During the
  `Busy` window the page fires 65 `readText` calls, collects the codes and writes them to
  `codes.txt`. Expected: 64 × `ok`, 1 × `EAGAIN`; today 65 × `ok` (red).

**Verification:** `cargo test -p lunarstorm-platform --test e2e_hostapi`
**Dependencies:** Task 6 · **Files:** `platform/desktop/app/src/ui/mod.rs`,
`platform/desktop/app/tests/e2e_hostapi.rs` · **Scope:** S

### Checkpoint: Robustness
- [ ] Green with counts read: `lunarstorm-host-desktop`, `lunarstorm-platform` (all three e2e
  tests), `lunar-extension-hostapi`, `lunarstorm-web`, `lunarstorm-pack`; clippy clean on the
  touched crates.
- [ ] `lunarstorm-debug platform/desktop/examples/app` starts and logs "WebView connected over IPC".

### Phase 3: Design findings

#### Task 8: Every runtime answers every call (D9)

**Description:** The contract line in `platform/desktop/app/src/runtime/mod.rs` says every `call`
gets a `result`. `MockRuntime` answers `EUNAVAIL` with the call's id.

**Acceptance criteria:**
- [ ] New test in `src/runtime/tests.rs`: a `call` to `MockRuntime` yields a `result` with
  `EUNAVAIL` and the same id within 1 s. Today nothing arrives (red, by `recv_timeout`).

**Verification:** `cargo test -p lunarstorm-platform` · **Dependencies:** None ·
**Files:** `src/runtime/mock.rs`, `src/runtime/mod.rs`, `src/runtime/tests.rs` · **Scope:** S

#### Task 9: One list of areas (D10)

**Description:** `pub type Areas = (Fs,)` in `platform/desktop/host/src/lib.rs`; `spec()` and
`Extensions` use it. Refactor only.

**Acceptance criteria:**
- [ ] `the_spec_lists_the_fs_area` and the e2e tests green, unchanged.

**Verification:** `cargo test -p lunarstorm-host-desktop` · **Dependencies:** None ·
**Files:** `platform/desktop/host/src/lib.rs` · **Scope:** XS

#### Task 10: Write the call contract down (D6, D11, D12)

**Description:** Comments only, next to the code:
- `envelope.rs` module doc gets the one table of codes a page can see:
  - from the host: `EREFUSED ERELOAD ETIMEDOUT EAGAIN E2BIG EUNAVAIL`;
  - from `fs`: `EACCES ENOENT EINVAL ENOTDIR EIO E2BIG EAGAIN`;
  - `EFAIL`: the result is not JSON.

  It also notes that the dispatcher's `EINVAL`/`EPERM` checks are a second line the WebView
  cannot reach, that `writeText` is bounded by `MAX_MESSAGE` (1 MiB), and that after
  `ETIMEDOUT`/`ERELOAD` the operation may have run.
- `shell.js` records D12 (writable `__zebraReceive`).
- `ui/mod.rs` `Inbound::Call` records D12 (no modal block).
- `tests/e2e_hostapi.rs` header lists the three tests and the desktop-session requirement
  (macOS: a logged-in session; Linux: `xvfb-run`).

**Acceptance criteria:**
- [ ] Each point is present at the named site; behaviour unchanged; suites green.

**Verification:** `cargo test -p lunarstorm-platform` · **Dependencies:** Tasks 1–8 (codes
final) · **Files:** `platform/desktop/app/src/envelope.rs`, `src/shell/shell.js`, `src/ui/mod.rs`,
`tests/e2e_hostapi.rs` · **Scope:** XS

### Checkpoint: Complete
- [ ] For each defect (Tasks 1, 2, 3, 5, 6, 7, 8), copied from real output: test name, failing
  line, passing line, command.
- [ ] All five suites green on the final tree with counts; committed on `desktop-hostapi-e2e`,
  not pushed. `tasks/` stays uncommitted.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| The `Busy` window (Tasks 6, 7) is timing-based | Med | 3 s of backend work against page actions of milliseconds; the busy loop is JS in the backend (budget 5 s), never a sleep in the host |
| A red FIFO run hangs the suite | High | The Task 3 guard object, dropped before the backend, releases the reader (`O_NONBLOCK`, `ENXIO` ignored) |
| A hung network mount leaks a worker thread | Low | At most 8 per backend (D2); the backend and other calls keep working; `EAGAIN` once all 8 hang |
| Three e2e tests need a desktop session | Med | Header of `e2e_hostapi.rs` states it; macOS runs as-is, Linux under `xvfb-run`; never `#[ignore]` |
| D1 breaks an app whose backend read `permissions.fs` folders | Low | No such app in the repo (`platform/desktop/examples/app` declares no `fs`); `manifest.rs` doc states the scope |
| 16 MiB / 3 MiB test files slow the suite | Low | Written under `CARGO_TARGET_TMPDIR`, about 0.1 s each |
