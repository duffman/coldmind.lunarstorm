# IPC review fixes: tasks

Plan: `tasks/plan.md` (decisions D1–D13). Each defect: failing test first → fix → the same test green.

## Phase 1: Security
- [ ] T1 Links cannot widen a root: broken symlink (D5) + hard link nlink>1 (D4); 2 red tests (S)
- [ ] T2 Backend `node:fs` gets no `permissions.fs` folders; `Program.area_roots` (D1); red test in runtime/tests.rs (M)
- [ ] Checkpoint: host-desktop + platform green, clippy clean

## Phase 2: Robustness
- [ ] T3 Regular files only via O_NONBLOCK + fstat (D3); FIFO red test with release guard (S)
- [ ] T4 `fs` on worker threads, max 8, EAGAIN, results via Settler (D2); unit tests pin the structure (M)
- [ ] T5 MAX_TEXT via take() in fs.rs; MAX_RESULT in envelope.rs enforced in Bridge::send; 2 red tests (M)
- [ ] T6 Hello drops the form's calls (D8); e2e `a_reloaded_page_does_not_inherit_the_old_pages_calls`, `#two` marker (S)
- [ ] T7 MAX_CALLS 64 → EAGAIN (D7); e2e `a_page_cannot_queue_more_than_max_calls` (S)
- [ ] Checkpoint: five suites green with counts, three e2e tests; example app starts

## Phase 3: Design
- [ ] T8 Mock runtime answers `call` with EUNAVAIL (D9); red test (S)
- [ ] T9 `type Areas = (Fs,)` (D10) (XS)
- [ ] T10 Code table, 1 MiB write limit, maybe-executed, D12 notes, e2e session header (XS)
- [ ] Checkpoint: proof per defect (test, failing line, passing line, command); commit, no push
