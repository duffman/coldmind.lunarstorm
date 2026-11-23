//! HTTP/1.1 request framing and response-head serialization. No sockets, no JS.
//!
//! The parser works on a borrowed byte window and records **offsets**, never slices,
//! so the same code parses requests in place from the reactor's scratch buffer (the
//! zero-copy fast path) or from a connection's own buffer when a request spans reads.
//!
//! Limits (design decisions):
//! - head larger than [`MAX_HEAD`] is answered with 431;
//! - body larger than [`MAX_BODY`] is answered with 413;
//! - `Content-Length` together with `Transfer-Encoding` is answered with 400, the
//!   request-smuggling defence of RFC 9112 section 6.3;
//! - a transfer coding other than `chunked` is answered with 501.

use std::io::Write;
use std::mem::MaybeUninit;

/// Largest accepted request head (request line plus header fields), in bytes.
pub const MAX_HEAD: usize = 64 * 1024;
/// Largest accepted request body, in bytes.
pub const MAX_BODY: usize = 64 * 1024 * 1024;
/// Largest number of header fields in one request.
const MAX_HEADERS: usize = 100;
/// Longest accepted chunk-size or trailer line, in bytes.
const MAX_CHUNK_LINE: usize = 4096;

/// Request methods passed to JS as an index instead of a string, so the common
/// methods allocate nothing. Mirrored by `METHODS` in `js/bootstrap.js`.
pub const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"];

/// How the request body is delimited.
#[derive(Clone, Copy)]
pub enum Framed {
    /// No body.
    None,
    /// `Content-Length` bytes follow the head.
    Length(usize),
    /// `Transfer-Encoding: chunked`.
    Chunked,
}

/// A parsed request head. All spans are `(start, end)` byte offsets into the window
/// the head was parsed from.
#[derive(Clone, Copy)]
pub struct Head {
    /// Span of the method token.
    pub method: (u32, u32),
    /// Span of the request target.
    pub path: (u32, u32),
    /// Span of the raw header-field block; JS parses it lazily on first access.
    pub fields: (u32, u32),
    /// Length of the whole head including the blank line.
    pub len: u32,
    /// Body framing.
    pub body: Framed,
    /// Whether the connection may be reused after this exchange.
    pub keep_alive: bool,
    /// Whether the request is HTTP/1.0 (no chunked responses).
    pub http10: bool,
    /// Whether the client sent `Expect: 100-continue` and a body is expected.
    pub expect: bool,
}

/// Where a connection is in receiving its current request.
pub enum Phase {
    /// Waiting for a complete head.
    Head,
    /// Head parsed; waiting until the window holds `end` bytes (head plus body).
    Body(Head, usize),
    /// Head parsed; decoding a chunked body.
    Chunked(Head, Dechunk),
}

/// A complete request body.
pub enum Body {
    /// No body.
    None,
    /// Body bytes at `(start, end)` in the window.
    Span(usize, usize),
    /// Decoded chunked body.
    Owned(Vec<u8>),
}

/// A complete request.
pub struct Request {
    /// The parsed head.
    pub head: Head,
    /// The body.
    pub body: Body,
    /// Bytes of the window this request occupies.
    pub consumed: usize,
}

/// Result of one [`advance`] call.
pub enum Step {
    /// The window does not hold a complete request yet.
    NeedMore,
    /// Like `NeedMore`, and the client is waiting for `100 Continue`.
    Continue,
    /// A complete request starts at offset 0 of the window.
    Request(Request),
    /// The request is malformed or over a limit; answer with this status and close.
    Bad(u16),
}

/// Tries to take one complete request from the front of `buf`.
///
/// The window must always start at the first byte of the current request: the caller
/// may append to it between calls but must not drop bytes until a request is returned.
///
/// # Arguments
/// * `buf` - bytes received so far for the current request, possibly followed by more.
/// * `phase` - the connection's parse state; updated in place.
///
/// # Returns
/// What the caller should do next; see [`Step`].
pub fn advance(buf: &[u8], phase: &mut Phase) -> Step {
    loop {
        match phase {
            Phase::Head => {
                let head = match parse_head(buf) {
                    Ok(Some(head)) => head,
                    Ok(None) if buf.len() > MAX_HEAD => return Step::Bad(431),
                    Ok(None) => return Step::NeedMore,
                    Err(status) => return Step::Bad(status),
                };
                let len = head.len as usize;
                match head.body {
                    Framed::None => {
                        return Step::Request(Request { head, body: Body::None, consumed: len })
                    }
                    Framed::Length(n) if n > MAX_BODY => return Step::Bad(413),
                    Framed::Length(n) => {
                        *phase = Phase::Body(head, len + n);
                        if buf.len() < len + n {
                            return if head.expect { Step::Continue } else { Step::NeedMore };
                        }
                    }
                    Framed::Chunked => {
                        *phase = Phase::Chunked(head, Dechunk::new(len));
                        if head.expect && buf.len() == len {
                            return Step::Continue;
                        }
                    }
                }
            }
            Phase::Body(head, end) => {
                if buf.len() < *end {
                    return Step::NeedMore;
                }
                let (head, end) = (*head, *end);
                *phase = Phase::Head;
                return Step::Request(Request {
                    head,
                    body: Body::Span(head.len as usize, end),
                    consumed: end,
                });
            }
            Phase::Chunked(head, dechunk) => match dechunk.feed(buf) {
                Ok(false) => return Step::NeedMore,
                Err(status) => return Step::Bad(status),
                Ok(true) => {
                    let request = Request {
                        head: *head,
                        body: Body::Owned(std::mem::take(&mut dechunk.out)),
                        consumed: dechunk.pos,
                    };
                    *phase = Phase::Head;
                    return Step::Request(request);
                }
            },
        }
    }
}

/// Parses a request head.
///
/// # Arguments
/// * `buf` - window starting at the first byte of the request line.
///
/// # Returns
/// `Some(head)` when the head is complete, `None` when more bytes are needed.
///
/// # Errors
/// The HTTP status to answer with: 400 malformed, 431 too many header fields,
/// 501 unsupported transfer coding.
fn parse_head(buf: &[u8]) -> Result<Option<Head>, u16> {
    // Uninitialised header slots: httparse fills only the ones it uses, so the
    // 100-slot table costs nothing per request.
    let mut slots = [const { MaybeUninit::<httparse::Header<'_>>::uninit() }; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut []);
    let len = match request.parse_with_uninit_headers(buf, &mut slots) {
        Ok(httparse::Status::Complete(len)) => len,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(httparse::Error::TooManyHeaders) => return Err(431),
        Err(_) => return Err(400),
    };
    let base = buf.as_ptr() as usize;
    let span = |s: &[u8]| {
        let start = s.as_ptr() as usize - base;
        (start as u32, (start + s.len()) as u32)
    };
    let http10 = request.version == Some(0);
    let mut keep_alive = !http10;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    let mut expect = false;
    for header in request.headers.iter() {
        let name = header.name.as_bytes();
        // Dispatch on length first: most header fields are rejected by one integer compare.
        match name.len() {
            14 if name.eq_ignore_ascii_case(b"content-length") => {
                let value = parse_decimal(header.value).ok_or(400u16)?;
                if length.is_some_and(|previous| previous != value) {
                    return Err(400);
                }
                length = Some(value);
            }
            17 if name.eq_ignore_ascii_case(b"transfer-encoding") => {
                if !header.value.trim_ascii().eq_ignore_ascii_case(b"chunked") {
                    return Err(501);
                }
                chunked = true;
            }
            10 if name.eq_ignore_ascii_case(b"connection") => {
                if has_token(header.value, b"close") {
                    keep_alive = false;
                } else if has_token(header.value, b"keep-alive") {
                    keep_alive = true;
                }
            }
            6 if name.eq_ignore_ascii_case(b"expect") => {
                expect = header.value.trim_ascii().eq_ignore_ascii_case(b"100-continue");
            }
            _ => {}
        }
    }
    if chunked && length.is_some() {
        return Err(400);
    }
    let body = match (chunked, length) {
        (true, _) => Framed::Chunked,
        (false, Some(n)) if n > 0 => Framed::Length(n),
        _ => Framed::None,
    };
    let fields_start = request.headers.first().map_or(len as u32, |h| span(h.name.as_bytes()).0);
    Ok(Some(Head {
        method: span(request.method.unwrap_or("").as_bytes()),
        path: span(request.path.unwrap_or("").as_bytes()),
        fields: (fields_start, len as u32),
        len: len as u32,
        body,
        keep_alive,
        http10,
        expect: expect && !matches!(body, Framed::None),
    }))
}

/// Parses an unsigned decimal number with no sign, spaces or overflow.
///
/// # Arguments
/// * `value` - header value bytes.
///
/// # Returns
/// The number, or `None` when the value is not a plain decimal that fits `usize`.
fn parse_decimal(value: &[u8]) -> Option<usize> {
    let digits = value.trim_ascii();
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0usize, |acc, &b| {
        if b.is_ascii_digit() {
            acc.checked_mul(10)?.checked_add((b - b'0') as usize)
        } else {
            None
        }
    })
}

/// Tests whether a comma-separated header value contains a token, ignoring case.
///
/// # Arguments
/// * `value` - header value bytes.
/// * `token` - lowercase token to look for.
///
/// # Returns
/// True when one list element equals the token.
fn has_token(value: &[u8], token: &[u8]) -> bool {
    value.split(|b| *b == b',').any(|part| part.trim_ascii().eq_ignore_ascii_case(token))
}

/// State of a chunked-body decoder.
enum ChunkState {
    /// Expecting a chunk-size line.
    Size,
    /// Copying chunk data.
    Data,
    /// Expecting the line ending after chunk data.
    DataEnd,
    /// Skipping trailer lines until the blank line.
    Trailer,
}

/// Incremental decoder for `Transfer-Encoding: chunked` bodies.
///
/// It keeps its position in the window between calls, so a body that arrives in many
/// reads is decoded in one pass rather than rescanned from the start each time.
pub struct Dechunk {
    /// Offset in the window of the next undecoded byte.
    pos: usize,
    /// Bytes left in the chunk being copied.
    left: usize,
    /// Current state.
    state: ChunkState,
    /// Decoded body so far.
    out: Vec<u8>,
}

impl Dechunk {
    /// Creates a decoder.
    ///
    /// # Arguments
    /// * `start` - offset in the window where the chunked body begins.
    ///
    /// # Returns
    /// A decoder positioned at the first chunk-size line.
    fn new(start: usize) -> Self {
        Self { pos: start, left: 0, state: ChunkState::Size, out: Vec::new() }
    }

    /// Decodes as far as the window allows.
    ///
    /// # Arguments
    /// * `buf` - the whole request window; decoding resumes at the saved offset.
    ///
    /// # Returns
    /// `true` once the terminating chunk and trailers have been consumed.
    ///
    /// # Errors
    /// 400 for malformed chunk framing, 413 when the decoded body exceeds [`MAX_BODY`].
    fn feed(&mut self, buf: &[u8]) -> Result<bool, u16> {
        loop {
            match self.state {
                ChunkState::Size => {
                    let Some(line) = self.line(buf)? else { return Ok(false) };
                    let digits = line.split(|b| *b == b';').next().unwrap_or(&[]).trim_ascii();
                    if digits.is_empty() || digits.len() > 16 {
                        return Err(400);
                    }
                    let mut size = 0usize;
                    for &b in digits {
                        size = size << 4 | (b as char).to_digit(16).ok_or(400u16)? as usize;
                    }
                    if size == 0 {
                        self.state = ChunkState::Trailer;
                    } else if self.out.len() + size > MAX_BODY {
                        return Err(413);
                    } else {
                        self.left = size;
                        self.state = ChunkState::Data;
                    }
                }
                ChunkState::Data => {
                    let take = self.left.min(buf.len() - self.pos);
                    self.out.extend_from_slice(&buf[self.pos..self.pos + take]);
                    self.pos += take;
                    self.left -= take;
                    if self.left > 0 {
                        return Ok(false);
                    }
                    self.state = ChunkState::DataEnd;
                }
                ChunkState::DataEnd => {
                    let Some(line) = self.line(buf)? else { return Ok(false) };
                    if !line.is_empty() {
                        return Err(400);
                    }
                    self.state = ChunkState::Size;
                }
                ChunkState::Trailer => {
                    let Some(line) = self.line(buf)? else { return Ok(false) };
                    if line.is_empty() {
                        return Ok(true);
                    }
                }
            }
        }
    }

    /// Takes one line ending in LF (with optional CR) from the window.
    ///
    /// # Arguments
    /// * `buf` - the request window.
    ///
    /// # Returns
    /// The line without its ending, or `None` when the ending has not arrived.
    ///
    /// # Errors
    /// 400 when the line exceeds [`MAX_CHUNK_LINE`] without an ending.
    fn line<'a>(&mut self, buf: &'a [u8]) -> Result<Option<&'a [u8]>, u16> {
        let rest = &buf[self.pos..];
        match rest.iter().position(|b| *b == b'\n') {
            Some(at) => {
                self.pos += at + 1;
                Ok(Some(rest[..at].strip_suffix(b"\r").unwrap_or(&rest[..at])))
            }
            None if rest.len() > MAX_CHUNK_LINE => Err(400),
            None => Ok(None),
        }
    }
}

/// How a response body is delimited.
pub enum Framing {
    /// `content-length: n`.
    Length(usize),
    /// `transfer-encoding: chunked`.
    Chunked,
    /// Body runs until the connection closes (HTTP/1.0 streaming).
    Close,
    /// The status forbids a body (1xx, 204, 304).
    None,
}

/// Tests whether a status code forbids a response body.
///
/// # Arguments
/// * `status` - HTTP status code.
///
/// # Returns
/// True for 1xx, 204 and 304.
pub fn bodiless(status: u16) -> bool {
    status < 200 || status == 204 || status == 304
}

/// Appends a complete response head to `out`.
///
/// # Arguments
/// * `out` - destination; bytes are appended.
/// * `status` - HTTP status code.
/// * `fields` - caller's header fields, already serialized as `name: value\r\n` lines.
/// * `date` - the cached `date: ...\r\n` line.
/// * `framing` - body delimitation to announce.
/// * `close` - whether to announce `connection: close`.
pub fn write_head(
    out: &mut Vec<u8>,
    status: u16,
    fields: &str,
    date: &[u8],
    framing: Framing,
    close: bool,
) {
    if status == 200 {
        out.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
    } else {
        out.extend_from_slice(b"HTTP/1.1 ");
        push_decimal(out, status as usize);
        out.push(b' ');
        out.extend_from_slice(reason(status).as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(fields.as_bytes());
    out.extend_from_slice(date);
    match framing {
        Framing::Length(n) => {
            out.extend_from_slice(b"content-length: ");
            push_decimal(out, n);
            out.extend_from_slice(b"\r\n");
        }
        Framing::Chunked => out.extend_from_slice(b"transfer-encoding: chunked\r\n"),
        Framing::Close | Framing::None => {}
    }
    if close {
        out.extend_from_slice(b"connection: close\r\n");
    }
    out.extend_from_slice(b"\r\n");
}

/// Appends a decimal number without going through `fmt`.
///
/// # Arguments
/// * `out` - destination; digits are appended.
/// * `n` - the number.
fn push_decimal(out: &mut Vec<u8>, mut n: usize) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

/// Writes a chunk-size line (`<hex>\r\n`) for a chunked response.
///
/// # Arguments
/// * `line` - destination buffer.
/// * `n` - chunk length in bytes.
///
/// # Returns
/// Number of bytes of `line` that were written.
pub fn chunk_line(line: &mut [u8; 18], n: usize) -> usize {
    let digits = ((usize::BITS - n.leading_zeros()).div_ceil(4)).max(1) as usize;
    for (i, byte) in line.iter_mut().enumerate().take(digits) {
        *byte = b"0123456789abcdef"[(n >> ((digits - 1 - i) * 4)) & 15];
    }
    line[digits] = b'\r';
    line[digits + 1] = b'\n';
    digits + 2
}

/// Formats the `date` header line for a Unix time.
///
/// Runs at most once per second (the reactor caches the line), so it uses `fmt`.
///
/// # Arguments
/// * `secs` - seconds since the Unix epoch.
/// * `out` - receives exactly `date: Thu, 01 Jan 1970 00:00:00 GMT\r\n`.
pub fn write_date(secs: u64, out: &mut [u8; 37]) {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = secs / 86_400;
    let clock = secs % 86_400;
    // Civil-from-days (Howard Hinnant), valid for all dates after 1970.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    let _ = write!(
        &mut out[..],
        "date: {}, {:02} {} {:04} {:02}:{:02}:{:02} GMT\r\n",
        DAYS[(days % 7) as usize],
        day,
        MONTHS[(month - 1) as usize],
        year,
        clock / 3_600,
        clock / 60 % 60,
        clock % 60
    );
}

/// Reason phrase for a status code.
///
/// # Arguments
/// * `status` - HTTP status code.
///
/// # Returns
/// The standard phrase, or `Unknown` for codes outside the table.
fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}
