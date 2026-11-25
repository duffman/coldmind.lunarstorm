//! End-to-end tests: each one runs the real `lunarstorm` binary on a script and checks
//! what it prints, what it sends over real sockets and how it exits.

// Every test here runs scripts: only a binary with the `runtime` package has them.
#![cfg(feature = "runtime")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// Runs `lunarstorm -e <code>` to completion.
///
/// # Arguments
/// * `code` - module source.
/// * `args` - extra command-line arguments, visible as `process.argv.slice(2)`.
///
/// # Returns
/// The finished process: status, stdout and stderr.
fn eval(code: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lunar"))
        .arg("-e")
        .arg(code)
        .args(args)
        .output()
        .expect("spawn lunarstorm")
}

/// Returns a process's stdout as text.
///
/// # Arguments
/// * `output` - a finished process.
///
/// # Returns
/// Its stdout, lossily decoded.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Returns a process's stderr as text.
///
/// # Arguments
/// * `output` - a finished process.
///
/// # Returns
/// Its stderr, lossily decoded.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Creates an empty scratch directory unique to one test.
///
/// # Arguments
/// * `name` - test name, used in the directory name.
///
/// # Returns
/// The directory's path.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lunarstorm-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn scheduling_order_matches_node() {
    let out = eval(
        r#"
        const log = [];
        setTimeout(() => { log.push('timeout'); console.log(log.join(' ')); }, 20);
        setImmediate(() => { log.push('immediate1'); Promise.resolve().then(() => log.push('micro-between')); });
        setImmediate(() => log.push('immediate2'));
        process.nextTick(() => log.push('tick'));
        Promise.resolve().then(() => { log.push('micro'); process.nextTick(() => log.push('tick-after-micro')); });
        log.push('sync');
        "#,
        &[],
    );
    assert_eq!(
        stdout(&out),
        "sync tick micro tick-after-micro immediate1 micro-between immediate2 timeout\n",
        "stderr: {}",
        stderr(&out)
    );
    assert!(out.status.success());
}

#[test]
fn timers_fire_in_order_and_cancel() {
    let out = eval(
        r#"
        const log = [];
        const stale = setTimeout(() => log.push('never'), 1);
        clearTimeout(stale);
        setTimeout(() => log.push('b'), 10);
        setTimeout(() => log.push('a'), 5);
        // A stale id must not cancel the timer that reuses its slot.
        setTimeout((x, y) => log.push(x + y), 15, 'c', 'd');
        clearTimeout(stale);
        let n = 0;
        const every = setInterval(() => {
          log.push('i' + ++n);
          if (n === 3) { clearInterval(every); setTimeout(() => console.log(log.join(' ')), 30); }
        }, 2);
        "#,
        &[],
    );
    let text = stdout(&out);
    let seen: Vec<&str> = text.split_whitespace().collect();
    assert!(!seen.contains(&"never"), "cancelled timer fired: {text}");
    let at = |name: &str| seen.iter().position(|s| *s == name).unwrap_or_else(|| panic!("{name} missing in {text}"));
    assert!(at("a") < at("b") && at("b") < at("cd"), "wrong order: {text}");
    assert!(at("i1") < at("i2") && at("i2") < at("i3"), "wrong order: {text}");
    assert_eq!(seen.iter().filter(|s| s.starts_with('i')).count(), 3, "interval not cleared: {text}");
    assert!(out.status.success());
}

#[test]
fn tcp_echo_round_trip() {
    let out = eval(
        r#"
        import net from 'node:net';
        const big = 'x'.repeat(300000);
        const server = net.createServer((sock) => sock.on('data', (chunk) => sock.write(chunk)));
        server.listen(0, '127.0.0.1', () => {
          let got = '';
          const client = net.connect(server.address().port, 'localhost');
          client.write('ping:');            // queued until the connect completes
          client.on('connect', () => client.write(big));
          client.on('data', (chunk) => {
            got += chunk.toString();
            if (got.length === 5 + big.length) client.end();
          });
          client.on('close', (hadError) => {
            console.log(got.slice(0, 5), got.length, got.endsWith('xxx'), hadError);
            server.close(() => console.log('closed'));
          });
        });
        "#,
        &[],
    );
    assert_eq!(stdout(&out), "ping: 300005 true false\nclosed\n", "stderr: {}", stderr(&out));
    assert!(out.status.success());
}

#[test]
fn tcp_connect_failure_is_reported() {
    let out = eval(
        r#"
        import net from 'node:net';
        const probe = net.createServer();
        probe.listen(0, '127.0.0.1', () => {
          const port = probe.address().port;
          probe.close(() => {
            const sock = net.connect(port, '127.0.0.1');
            sock.on('error', (err) => console.log('error', err.code));
            sock.on('close', (hadError) => console.log('close', hadError));
          });
        });
        "#,
        &[],
    );
    assert_eq!(stdout(&out), "error ECONNREFUSED\nclose true\n", "stderr: {}", stderr(&out));
    assert!(out.status.success());
}

/// Server used by the HTTP test: sync, async-streamed and body-echoing routes.
const HTTP_SERVER: &str = r#"
import http from 'node:http';
const server = http.createServer(async (req, res) => {
  // close() first: the response is then in flight on a closing server and must
  // announce `connection: close`.
  if (req.url === '/quit') { server.close(); res.end('bye'); return; }
  if (req.method === 'POST') {
    const parts = [];
    for await (const chunk of req) parts.push(chunk);
    res.setHeader('Content-Type', 'text/plain');
    res.end('got[' + Buffer.concat(parts).toString() + ']');
    return;
  }
  if (req.url === '/stream') {
    setTimeout(() => { res.writeHead(201, { 'x-kind': 'stream' }); res.write('ab'); res.write('cde'); res.end(); }, 5);
    return;
  }
  res.setHeader('X-Host', req.headers.host);
  res.end(req.method + ' ' + req.url);
});
server.listen(0, '127.0.0.1', () => console.log(server.address().port));
"#;

#[test]
fn http_keep_alive_pipelining_bodies_and_streaming() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lunar"))
        .arg("-e")
        .arg(HTTP_SERVER)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lunarstorm");
    let mut lines = BufReader::new(child.stdout.take().expect("child stdout"));
    let mut port = String::new();
    lines.read_line(&mut port).expect("read port");
    let port: u16 = port.trim().parse().expect("port number");

    // Six requests on one connection, written at once: two pipelined GETs, a HEAD,
    // a Content-Length body, a chunked body, an async chunked response, then quit.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("set timeout");
    stream
        .write_all(
            b"GET /one HTTP/1.1\r\nHost: a.test\r\n\r\n\
              GET /two HTTP/1.1\r\nHost: b.test\r\n\r\n\
              HEAD /head HTTP/1.1\r\nHost: c.test\r\n\r\n\
              POST /len HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\n\r\nhello\
              POST /chunked HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nfoo\r\n4\r\nbarz\r\n0\r\n\r\n\
              GET /stream HTTP/1.1\r\nHost: t\r\n\r\n\
              GET /quit HTTP/1.1\r\nHost: t\r\n\r\n",
        )
        .expect("write requests");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read responses until close");

    let responses: Vec<&str> = raw.split("HTTP/1.1 ").skip(1).collect();
    assert_eq!(responses.len(), 7, "expected 7 responses, got: {raw}");
    let body = |i: usize| responses[i].split_once("\r\n\r\n").expect("header end").1;
    let head = |i: usize| responses[i].split_once("\r\n\r\n").expect("header end").0.to_ascii_lowercase();

    assert!(responses[0].starts_with("200 OK\r\n"));
    assert!(head(0).contains("x-host: a.test"), "{raw}");
    assert!(head(0).contains("content-length: 8"), "{raw}");
    assert!(head(0).contains("date: "), "{raw}");
    assert_eq!(body(0), "GET /one");
    assert!(head(1).contains("x-host: b.test"), "{raw}");
    assert_eq!(body(1), "GET /two");
    assert!(head(2).contains("content-length: 10"), "{raw}");
    assert_eq!(body(2), "", "HEAD must not carry a body");
    assert_eq!(body(3), "got[hello]");
    assert!(head(3).contains("content-type: text/plain"), "{raw}");
    assert_eq!(body(4), "got[foobarz]");
    assert!(responses[5].starts_with("201 Created\r\n"), "{raw}");
    assert!(head(5).contains("transfer-encoding: chunked") && head(5).contains("x-kind: stream"), "{raw}");
    assert_eq!(body(5), "2\r\nab\r\n3\r\ncde\r\n0\r\n\r\n");
    assert!(head(6).contains("connection: close"), "{raw}");
    assert_eq!(body(6), "bye");

    let status = child.wait().expect("wait for lunarstorm");
    assert!(status.success(), "server did not exit cleanly after close()");
}

#[test]
fn http_rejects_malformed_and_smuggling_requests() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lunar"))
        .arg("-e")
        .arg(HTTP_SERVER)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lunarstorm");
    let mut lines = BufReader::new(child.stdout.take().expect("child stdout"));
    let mut port = String::new();
    lines.read_line(&mut port).expect("read port");
    let port: u16 = port.trim().parse().expect("port number");

    let exchange = |request: &[u8]| {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(10))).expect("set timeout");
        stream.write_all(request).expect("write request");
        let mut raw = String::new();
        stream.read_to_string(&mut raw).expect("read until close");
        raw
    };
    assert!(exchange(b"NOT HTTP\r\n\r\n").starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(exchange(
        b"POST / HTTP/1.1\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
    )
    .starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(exchange(b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n")
        .starts_with("HTTP/1.1 501 Not Implemented\r\n"));
    // HTTP/1.0 without keep-alive: answered, then closed by the server.
    let old = exchange(b"GET /old HTTP/1.0\r\nHost: t\r\n\r\n");
    assert!(old.starts_with("HTTP/1.1 200 OK\r\n") && old.ends_with("GET /old"), "{old}");

    assert!(exchange(b"GET /quit HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").ends_with("bye"));
    assert!(child.wait().expect("wait for lunarstorm").success());
}

#[test]
fn fs_sync_async_and_errors() {
    let dir = scratch("fs");
    let out = eval(
        r#"
        import fs from 'node:fs';
        import { readFile, writeFile, stat, readdir, rm } from 'node:fs/promises';
        import path from 'node:path';
        const dir = process.argv[2];
        const file = path.join(dir, 'sub', 'a.txt');
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, 'héllo');
        fs.appendFileSync(file, Buffer.from(' wörld'));
        console.log(fs.readFileSync(file, 'utf8'), fs.readFileSync(file).length, fs.existsSync(file));
        await writeFile(path.join(dir, 'b.bin'), new Uint8Array([1, 2, 3]));
        const [text, bytes, info, names] = await Promise.all([
          readFile(file, { encoding: 'utf8' }), readFile(path.join(dir, 'b.bin')), stat(file), readdir(dir),
        ]);
        console.log(text, Buffer.isBuffer(bytes), [...bytes].join(''), info.size, info.isFile(), names.sort().join(','));
        try { await readFile(path.join(dir, 'missing')); } catch (err) { console.log(err.code, err.message.startsWith('ENOENT: no such file')); }
        try { fs.readFileSync(path.join(dir, 'missing')); } catch (err) { console.log(err.code); }
        fs.readFile(file, 'utf8', (err, data) => {
          console.log(err, data.length);
          rm(dir, { recursive: true }).then(() => console.log(fs.existsSync(dir)));
        });
        "#,
        &[dir.to_str().expect("utf-8 temp path")],
    );
    assert_eq!(
        stdout(&out),
        "héllo wörld 13 true\nhéllo wörld true 123 13 true b.bin,sub\nENOENT true\nENOENT\nnull 11\nfalse\n",
        "stderr: {}",
        stderr(&out)
    );
    assert!(out.status.success());
}

#[test]
fn modules_import_files_and_builtins() {
    let dir = scratch("modules");
    std::fs::write(dir.join("lib.js"), "export const twice = (n) => n * 2;\nexport default import.meta.main;\n")
        .expect("write lib");
    std::fs::write(
        dir.join("main.mjs"),
        "import isMain, { twice } from './lib';\n\
         import EventEmitter, { EventEmitter as Named } from 'events';\n\
         import { basename } from 'node:path';\n\
         const lazy = await import('./lib.js');\n\
         const bus = new EventEmitter();\n\
         bus.once('hi', (a, b, c, d) => console.log(a + b + c + d));\n\
         bus.emit('hi', 1, 2, 3, 4); bus.emit('hi', 9, 9, 9, 9);\n\
         console.log(twice(21), isMain, import.meta.main, lazy.twice === twice, Named === EventEmitter,\n\
           basename(import.meta.filename), process.argv.slice(2));\n",
    )
    .expect("write main");
    let out = Command::new(env!("CARGO_BIN_EXE_lunar"))
        .arg(dir.join("main.mjs"))
        .args(["x", "y"])
        .output()
        .expect("spawn lunarstorm");
    assert_eq!(stdout(&out), "10\n42 false true true true main.mjs [ 'x', 'y' ]\n", "stderr: {}", stderr(&out));
    assert!(out.status.success());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn failures_set_the_exit_status() {
    let thrown = eval("console.log('before'); throw new TypeError('boom');", &[]);
    assert_eq!(thrown.status.code(), Some(1));
    assert_eq!(stdout(&thrown), "before\n");
    assert!(stderr(&thrown).contains("TypeError: boom"), "{}", stderr(&thrown));

    let late = eval("setTimeout(() => { throw new Error('late'); }, 1);", &[]);
    assert_eq!(late.status.code(), Some(1));
    assert!(stderr(&late).contains("Uncaught Error: late"), "{}", stderr(&late));

    let rejected = eval("Promise.reject(new Error('nope'));", &[]);
    assert_eq!(rejected.status.code(), Some(1));
    assert!(stderr(&rejected).contains("Uncaught (in promise) Error: nope"), "{}", stderr(&rejected));

    let handled = eval("const p = Promise.reject(new Error('x')); p.catch(() => console.log('caught'));", &[]);
    assert_eq!(handled.status.code(), Some(0));
    assert_eq!(stdout(&handled), "caught\n");

    let exited = eval("console.log('out'); process.exit(3); console.log('unreachable');", &[]);
    assert_eq!(exited.status.code(), Some(3));
    assert_eq!(stdout(&exited), "out\n");

    let coded = eval("process.exitCode = 4;", &[]);
    assert_eq!(coded.status.code(), Some(4));

    let syntax = eval("let = ;", &[]);
    assert_eq!(syntax.status.code(), Some(1));
    assert!(stderr(&syntax).contains("SyntaxError"), "{}", stderr(&syntax));

    let missing = eval("import x from 'left-pad';", &[]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("left-pad"), "{}", stderr(&missing));
}

#[test]
fn console_formats_values() {
    let out = eval(
        r#"
        const cycle = { name: 'c' }; cycle.self = cycle;
        console.log('%s is %d%%', 'load', 42, { a: [1, 'two', null], 'b-c': undefined });
        console.log(new Map([['k', 1]]), new Set([1]), cycle, [], {}, 10n, Buffer.from('hi'));
        console.error('to stderr');
        "#,
        &[],
    );
    assert_eq!(
        stdout(&out),
        "load is 42% { a: [ 1, 'two', null ], 'b-c': undefined }\n\
         Map(1) { 'k' => 1 } Set(1) { 1 } { name: 'c', self: [Circular] } [] {} 10n Buffer(2) [ 104, 105 ]\n",
        "stderr: {}",
        stderr(&out)
    );
    assert_eq!(stderr(&out), "to stderr\n");
}

/// Pins what scripts can observe of promises and `process.nextTick`, because the
/// engine's promise code and the tick queue are tuned for speed: resolving
/// functions are created in a prepared shape, a primitive is resolved without
/// them, and ticks are drained from one reused array. The expected output is what
/// Node.js 24 prints for the same script.
#[test]
fn promise_and_tick_semantics_match_node() {
    let out = eval(
        r#"
        const log = (...xs) => console.log(xs.join(' '));
        // 1. resolving functions look the same from JS
        let res, rej;
        new Promise((a, b) => { res = a; rej = b; });
        const shape = (f) => JSON.stringify([f.length, f.name, Object.getOwnPropertyNames(f), Object.getOwnPropertyDescriptor(f, 'length'), Object.getOwnPropertyDescriptor(f, 'name')]);
        log(shape(res));
        log(shape(rej), Object.getPrototypeOf(res) === Function.prototype);
        delete res.name; res.extra = 1;
        log(JSON.stringify(Object.getOwnPropertyNames(res)), JSON.stringify(Object.getOwnPropertyNames(rej)), res.name === '', rej.name === '');
        // 2. a primitive is never treated as a thenable
        Number.prototype.then = function (resolve) { log('number then called'); resolve('hijacked'); };
        log(await 5, await Promise.resolve(6), typeof (await undefined), await null, await 'text', await 7n, typeof (await Symbol.iterator));
        delete Number.prototype.then;
        // 3. objects are: thenables are followed, subclasses are constructed
        log(await { then(resolve) { resolve('thenable'); } });
        class Sub extends Promise { constructor(executor) { log('Sub constructed'); super(executor); } }
        const sub = Sub.resolve(1);
        log(sub instanceof Sub, await sub);
        // 4. Promise.resolve of a primitive
        const direct = Promise.resolve('x');
        log(direct instanceof Promise, direct.constructor === Promise, Promise.resolve(direct) === direct, await direct);
        try { Promise.resolve.call(1, 2); } catch (error) { log(error instanceof TypeError); }
        log(await Promise.reject(3).catch((reason) => 'rejected ' + reason));
        // 5. an await of a primitive takes exactly one microtask turn
        const order = [];
        const body = (async () => { order.push('a1'); await null; order.push('a2'); await 1; order.push('a3'); })();
        Promise.resolve().then(() => order.push('p1')).then(() => order.push('p2')).then(() => order.push('p3'));
        await body; await null; await null;
        log(order.join(' '));
        // 6. nextTick: arguments, order, and ticks queued by ticks run before promise jobs
        const ticks = [];
        process.nextTick((...xs) => ticks.push('args:' + xs.length), 1, undefined);
        process.nextTick((...xs) => ticks.push('none:' + xs.length));
        process.nextTick(() => {
          ticks.push('A');
          process.nextTick(() => { ticks.push('A-child'); process.nextTick(() => ticks.push('A-grandchild')); });
          Promise.resolve().then(() => { ticks.push('micro'); process.nextTick(() => ticks.push('tick-after-micro')); });
        });
        process.nextTick(() => ticks.push('B'));
        await new Promise((resolve) => setTimeout(resolve, 5));
        log(ticks.join(' '));
        "#,
        &[],
    );
    let expected = r#"[1,"",["length","name"],{"value":1,"writable":false,"enumerable":false,"configurable":true},{"value":"","writable":false,"enumerable":false,"configurable":true}]
[1,"",["length","name"],{"value":1,"writable":false,"enumerable":false,"configurable":true},{"value":"","writable":false,"enumerable":false,"configurable":true}] true
["length","extra"] ["length","name"] true true
5 6 undefined  text 7 symbol
thenable
Sub constructed
Sub constructed
true 1
true true true x
true
rejected 3
a1 a2 p1 a3 p2 p3
args:2 none:0 A B A-child A-grandchild micro tick-after-micro
"#;
    assert_eq!(stdout(&out), expected, "stderr: {}", stderr(&out));
    assert!(out.status.success());
}

/// A JSON import with `with { type: "json" }` runs. Import attributes now
/// reach the engine; Lunarstorm's module graph ignores them (the host picks a
/// module's kind from its file) instead of rejecting the import.
#[test]
fn json_import_with_attributes_runs() {
    let dir = scratch("import-attributes");
    std::fs::write(dir.join("d.json"), r#"{"a":42}"#).expect("write json");
    let main = dir.join("main.mjs");
    std::fs::write(
        &main,
        "import d from './d.json' with { type: 'json' };\nconsole.log(d.a);\n",
    )
    .expect("write module");
    let out = Command::new(env!("CARGO_BIN_EXE_lunar"))
        .arg(&main)
        .output()
        .expect("spawn lunarstorm");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "42\n");
}

/// Fix: a script of about a thousand member assignments failed to load with
/// "capacity: verification state budget/empty code", because every member
/// assignment took a new local in the Rust compiler. Long top-level code must
/// run through the real binary.
#[test]
fn long_script_of_member_assignments_runs() {
    let dir = scratch("long-script");
    let main = dir.join("main.js");
    let mut source = String::from("var o={};");
    for i in 0..5000 {
        source.push_str(&format!("o.a{i}=1;o.b{i}+=1;o.c{i}||=1;o.a{i}++;"));
    }
    source.push_str("console.log(Object.keys(o).length);");
    std::fs::write(&main, source).expect("write script");
    let out = Command::new(env!("CARGO_BIN_EXE_lunar"))
        .arg(&main)
        .output()
        .expect("spawn lunarstorm");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "15000\n");
}
