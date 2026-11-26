# Native image resource limits

`image::ImageLimits` is a tighten-only caller policy. Every limit must be no greater than its default; an attempted increase or exhaustion returns an error beginning `capacity:`. Structural/semantic invalidity remains a separate error. Existing entrypoints use defaults, and the `_with_limits` variants accept an explicit policy. Native bytecode, compiler profiles and ordinary artifact bytes are unchanged by these limits.

| Field | Default | Scope/rationale |
| --- | ---: | --- |
| `max_native_bytes` | 16 MiB | Native payload and aggregate graph code; existing wire/loader bound. The encoder stops output growth at this boundary. |
| `max_graph_bytes` | 64 MiB | Charged owned/decoded representation, including protected span arrays. Decoder atom/string/table allocations are charged before construction. |
| `max_functions` | 65,535 | Whole graph, including root; existing native image policy. |
| `max_depth` | 256 | Nested function edges; root is depth zero. Also bounds metadata paths. |
| `max_table_entries` | 65,535 | Per-function arguments, locals, references, captures/constants/atoms; the combined vardef array can contain arguments + locals. Also bounds the global native atom table and module tables. Native counts use 16-bit slots. |
| `max_instructions` | 1,000,000 | Per function; existing raw `Limits` policy. |
| `max_positions` | 1,000,000 | Per function, including encoded native location records and protected span entries. |
| `max_state_cells` | 8,000,000 | Aggregate `instruction_count × (local_count + 1)` over the graph. A logical cell accounts for both initialization and activity facts; these are stored in two byte vectors, not an 8 MiB process-memory promise. |
| `max_work` | 64,000,000 | Charged parse/hash bytes, representation visits, instruction/CFG visits, fact-vector cloning/merges and scope-chain steps. Explicit resource policy, not a JavaScript syntax limit or a wall-clock deadline. |
| `max_artifact_bytes` | 64 MiB | Protected envelope bound; exact serialized size is checked before its allocation. |

Existing additional representation caps remain: UTF16 strings/sources have at most 8 Mi code units, BigInt constants at most 1 MiB of canonical signed bytes, filename length less than 16,384 UTF8 bytes, and protected source-span storage at most 16 MiB. These limits are checked before expensive conversion/allocation where their lengths are available.

Owned graphs receive an iterative representation preflight before semantic instruction/state allocation. It checks table lengths, graph depth/count, strings, code widths/instruction counts and aggregate state cells. The semantic worklist shares the remaining work budget and de-duplicates queued states. Native decoding validates count/byte headers and a representation-derived state lower bound before allocating the corresponding data; after reading code it counts instructions without allocating instruction metadata before checking aggregate state cells. All constructors, ordinary/module/eval decoding and protected artifact ingress use the same policy. Work consumed by one phase is subtracted before the next phase. Protected spans and native graphs share the allocation allowance rather than receiving independent full allowances.

Graph bytes are charged representation storage, not process RSS: allocator/tree overhead, the caller's input, externally owned lexical layouts, native VM allocations, compiler AST/IR and worker process lifetime have separate owners/policies. There is no claim that this crate alone enforces a whole compiler-job memory or time limit.

## Protected eval responses

`VerifiedEvalImage::serialize_artifact[_with_limits]` and `eval::deserialize_eval_artifact[_with_limits]` use a distinct `QJSREVL` envelope with the existing envelope version, target/compiler profile, complete native payload, protected `SpanMetadata`, environment digest and SHA256 integrity. The native bytes still enter the existing eval verifier against the caller's immutable `VerifiedEnvLayout`. Ordinary artifact loading does not accept this magic or admit root captures. The envelope contains no native frame lease, pointer or engine value. Integrity is not publisher authentication; outer package/request identity policy remains required.
