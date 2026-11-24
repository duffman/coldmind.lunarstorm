# lunarstorm-http1 (crates/http1)

HTTP/1.1 on bytes: request framing and response-head serialization, with no sockets, no event loop and no JS. It sits in `crates/` rather than inside the Runtime so any other server in the monorepo can reuse it; today only `runtime/core` does (its `http` module). Parsing wraps `httparse` and records byte offsets, never slices, so one code path serves both a request parsed in place from the reactor's 64 KiB scratch buffer and one that spans reads in a connection's own buffer.

## Public surface

| item | role |
|---|---|
| `advance(buf, &mut Phase) -> Step` | take one complete request from the front of `buf`; `Step::{NeedMore, Continue, Request(Request), Bad(u16)}` |
| `Phase` | per-connection parse state: `Head`, `Body(Head, end)`, `Chunked(Head, Dechunk)` |
| `Head`, `Request`, `Body`, `Framed` | offsets of method, target and raw header block; `keep_alive`, `http10`, `expect` flags; body as `None`, `Span(start, end)` or `Owned(Vec<u8>)` |
| `Dechunk` | incremental `Transfer-Encoding: chunked` decoder; keeps its position between reads |
| `write_head(out, status, fields, date, Framing, close)` | append a full response head; `Framing::{Length(n), Chunked, Close, None}` |
| `bodiless(status)`, `chunk_line`, `write_date` | 1xx/204/304 test, `<hex>\r\n` chunk-size line, cached `date:` line |
| `METHODS`, `MAX_HEAD`, `MAX_BODY` | method index table, 64 KiB head limit, 64 MiB body limit |

The caller owns the window: it must always start at the first byte of the current request, and may append between calls but not drop bytes until `Step::Request` returns.

## How it fits

- Dependency: `httparse` only. Used by `runtime/core` (`io.rs` writes responses, `runtime.rs` calls `advance`).
- Part of the Runtime's `net`/`http` bindings in doc/plans/main.md §5; the module is listed in the root `Cargo.toml` header.

## Build and test

```text
cargo build -p lunarstorm-http1
cargo test -p lunarstorm-http1
```

The crate has no unit tests; the framing, keep-alive, pipelining and smuggling rejections are exercised through the real binary in `apps/lunarstorm/tests/cli.rs` (`http_keep_alive_pipelining_bodies_and_streaming`, `http_rejects_malformed_and_smuggling_requests`: `cargo test -p lunarstorm --test cli http_`).

## Decisions and hazards

- Limits: head over `MAX_HEAD` or over 100 header fields gets 431; body over `MAX_BODY` gets 413; chunk-size and trailer lines over 4096 bytes get 400.
- `Content-Length` together with `Transfer-Encoding` is a 400 (request-smuggling defence, RFC 9112 section 6.3); any transfer coding other than `chunked` is 501. A `Content-Length` must be a plain decimal that fits `usize` (no sign, no spaces, no overflow).
- `Step::Bad(status)` means answer with that status and close the connection; the stream cannot be resynchronized.
- `Step::Continue` is `NeedMore` plus "client sent `Expect: 100-continue`": the caller must write `100 Continue` itself.
- `METHODS` is mirrored by `METHODS` in `runtime/core/src/js/bootstrap.js` (methods cross into JS as an index so common ones allocate nothing). Their order must match. That file's comment still names `src/http.rs`, which no longer exists.
- Header fields are not parsed here: `Head.fields` is the raw block and JS parses it lazily on first access. The response `fields` argument must already be serialized `name: value\r\n` lines.
- Numbers are appended without `fmt`, and the header table is 100 uninitialised slots, to keep the per-request cost flat.
