#ifndef JETBUNNY_QUICKJS_NG_BRIDGE_H
#define JETBUNNY_QUICKJS_NG_BRIDGE_H
#include <stdint.h>
#include <stddef.h>
#include "jetbunny-eval.h"
#include "jetbunny-inspect.h"
/* All input values borrowed, all returned values owned (except argv_get).
 * Caller is the sole owner thread, keeps context live, releases values before close.
 * No callback may unwind. Finalizers must not execute JS. */
typedef struct jbq_context jbq_context;
typedef struct { uint64_t words[2]; } jbq_value;
typedef struct jbq_keys jbq_keys;
typedef int (*jbq_interrupt)(void *opaque);
typedef void (*jbq_drop)(void *opaque);
/* argv is native, borrowed and only valid during callback; use argv_get.
 * Returned value owned; thrown: 0 success, 1 throw returned value, 2 host fault.
 * Metadata errors/panics must also be retained by Rust out-of-band. */
typedef jbq_value (*jbq_host)(void *opaque, jbq_context *, jbq_value receiver,
                            const void *argv, size_t argc, int *thrown);
/* runtime: a JSRuntime the caller owns and frees after jbq_context_free. */
jbq_context *jbq_context_new(void *runtime, size_t memory_limit, size_t stack_limit,
                             jbq_interrupt interrupt, void *interrupt_opaque);
void jbq_context_free(jbq_context *);
/* lunarstorm patch: the bridge ABI revision; the Rust side (lunarstorm-debug ffi.rs
 * JBQ_ABI) refuses to attach on a mismatch. Decision: it stays jetbunny's 14.
 * Lunarstorm only adds functions (jbq_abi, jbq_context_wrap, jbq_debug_poll,
 * jbq_debug_rejection, jbq_set_base_compiler), and a missing function fails at
 * link time; no jetbunny declaration changed. Bump it with the first change to an
 * existing declaration's signature or to a shared struct's layout. */
#define JBQ_ABI 14u
uint32_t jbq_abi(void);
/* lunarstorm patch: the bridge around a context its owner (rquickjs) created.
 * The context is duplicated, never replaced; the runtime's interrupt handler,
 * rejection tracker, dynamic-import function and limits stay the owner's, and
 * jbq_context_free leaves them in place. NULL on allocation failure. */
jbq_context *jbq_context_wrap(void *js_context);
/* lunarstorm patch: the debugger's share of the interrupt poll for a wrapped
 * context, which the owner's interrupt handler calls: 0 continue, nonzero stop. */
int jbq_debug_poll(jbq_context *);
/* lunarstorm patch: forwards one rejection-tracker event of a wrapped context to
 * the debugger; promise and reason point at the engine's JSValues (borrowed). */
void jbq_debug_rejection(jbq_context *, void *promise, void *reason, int handled);
/* The JSContext with one new reference for the caller. */
void *jbq_js_context(jbq_context *);
void jbq_set_entry(jbq_context *, void *entry);
void *jbq_get_entry(jbq_context *);
void jbq_gc(jbq_context *);
/**
 * Reads the runtime heap's size and ceiling (documented in bridge.c).
 * @param used Receives the allocated bytes including allocator overhead.
 * @param limit Receives the memory limit; 0 when there is none.
 */
void jbq_heap_usage(jbq_context *, int64_t *used, int64_t *limit);
/* Nonallocating sticky native OOM take/reset; independent of exception value. */
int jbq_take_oom(jbq_context *);
/* Address of that sticky flag (one byte, nonzero when set), valid until context_free. */
const unsigned char *jbq_oom_flag(jbq_context *);
/* Private fault shaping/native policy hook; does not enter JavaScript. */
void jbq_set_memory_limit(jbq_context *, size_t);
const char *jbq_version(void);
jbq_value jbq_dup(jbq_context *, jbq_value);
void jbq_free(jbq_context *, jbq_value);
jbq_value jbq_global(jbq_context *);
jbq_value jbq_undefined(void);
jbq_value jbq_null(void);
jbq_value jbq_bool(jbq_context *, int);
jbq_value jbq_number(jbq_context *, double);
jbq_value jbq_string(jbq_context *, const uint16_t *, size_t);
jbq_value jbq_bytes(jbq_context *, const uint8_t *, size_t);
jbq_value jbq_array(jbq_context *);
jbq_value jbq_record(jbq_context *);
/* kind: undefined 0,null 1,bool 2,number 3,string 4,symbol 5,bigint 6,object 7,function 8. */
int jbq_kind(jbq_context *, jbq_value);
int jbq_is_exception(jbq_value);
/* Non-coercing inspection of a thrown value, not the exception sentinel. */
int jbq_is_uncatchable(jbq_value);
jbq_value jbq_take_exception(jbq_context *);
int jbq_to_bool(jbq_context *, jbq_value);
int jbq_to_number(jbq_context *, jbq_value, double *);
/* string_data only accepts primitives; no coercion/application execution. */
const uint16_t *jbq_string_data(jbq_context *, jbq_value, size_t *);
void jbq_string_free(jbq_context *, const uint16_t *);
/* string_utf8 only accepts primitives; no coercion. Unpaired surrogates stay WTF-8 bytes
 * (invalid UTF-8), so the caller can tell them apart. Release with utf8_free. */
const char *jbq_string_utf8(jbq_context *, jbq_value, size_t *);
void jbq_utf8_free(jbq_context *, const char *);
/* A new string from UTF-8 bytes. */
jbq_value jbq_string_from_utf8(jbq_context *, const char *, size_t);
/* An owned atom for a UTF-16 property name; 0 with a pending exception on failure. */
uint32_t jbq_atom(jbq_context *, const uint16_t *, size_t);
void jbq_atom_free(jbq_context *, uint32_t);
/* get/set by a borrowed atom from jbq_atom; semantics as jbq_get/jbq_set. */
jbq_value jbq_get_atom(jbq_context *, jbq_value object, uint32_t atom);
int jbq_set_atom(jbq_context *, jbq_value object, uint32_t atom, jbq_value value);
jbq_value jbq_get(jbq_context *, jbq_value object, jbq_value primitive_key);
int jbq_set(jbq_context *, jbq_value object, jbq_value primitive_key, jbq_value value);
jbq_value jbq_get_index(jbq_context *, jbq_value, uint32_t);
int jbq_define_index(jbq_context *, jbq_value, uint32_t, jbq_value);
int jbq_has_own_index(jbq_context *, jbq_value, uint32_t);
int jbq_length(jbq_context *, jbq_value, int64_t *);
int jbq_define_key(jbq_context *, jbq_value, jbq_value primitive_key, jbq_value);
jbq_value jbq_call(jbq_context *, jbq_value function, jbq_value receiver, const jbq_value *, size_t);
jbq_value jbq_construct(jbq_context *, jbq_value function, jbq_value new_target, const jbq_value *, size_t);
/* Consumes opaque exactly once on ALL success/failure paths. */
jbq_value jbq_host_function(jbq_context *, jbq_value name, uint16_t arity,
                             jbq_host, void *opaque, jbq_drop);
/* Borrowed native argv element, duplicate before placing in an owning store. */
jbq_value jbq_argv_get(const void *argv, size_t index);
/* Returns promise and writes owned hidden resolver pair; pair undefined on failure. */
jbq_value jbq_deferred(jbq_context *, jbq_value *pair);
jbq_value jbq_settle(jbq_context *, jbq_value pair, int reject, jbq_value);
/* The runtime's own job list, used only while no job sink is set (the C smoke test). */
int jbq_job_pending(jbq_context *);
/* 0 no job, 1 executed, -1 exception in this context. */
int jbq_execute_job(jbq_context *);
/* jetbunny-host-jobs-v1: the host owns the job queue. The sink receives every job the
 * runtime creates, in creation order; < 0 refuses it and the enqueue throws. The host runs
 * each job once with jbq_run_job or releases it unrun with jbq_drop_job, before the
 * context is freed. jbq_context_free clears the sink first. */
typedef struct jbq_job jbq_job;
typedef int (*jbq_job_sink)(void *opaque, jbq_job *job);
void jbq_set_job_sink(jbq_context *, jbq_job_sink, void *opaque);
/* Runs and releases the job: 1 ran, -1 exception in this context. */
int jbq_run_job(jbq_context *, jbq_job *);
void jbq_drop_job(jbq_context *, jbq_job *);
/* Queues function() as a job in FIFO order with promise jobs, under the async context
 * current now. 0 queued, -1 exception (not callable, out of memory). */
int jbq_enqueue_job(jbq_context *, jbq_value function);
/* The async context promise reactions capture; owned result, undefined until set. */
jbq_value jbq_async_context(jbq_context *);
void jbq_set_async_context(jbq_context *, jbq_value);
/* Takes the oldest rejection-tracker event: 0 none, 1 unhandled, 2 handled later.
 * On 1 or 2 writes owned promise and reason. Events are never dropped; the queue grows
 * within the runtime memory limit and -1 reports an allocation failure recorded earlier. */
int jbq_take_rejection(jbq_context *, jbq_value *promise, jbq_value *reason);
/* Detaches an ArrayBuffer: 0 detached, -1 exception (not an ArrayBuffer). */
int jbq_detach_buffer(jbq_context *, jbq_value);
int jbq_internals(jbq_context *, jbq_value, jbq_value *first, jbq_value *second);
jbq_value jbq_eval(jbq_context *, const uint16_t *, size_t, const char *origin);
jbq_value jbq_compile(jbq_context *, const uint16_t *, size_t, const char *origin);
jbq_value jbq_compile_debug(jbq_context *, const uint16_t *, size_t, const char *origin);
uint8_t *jbq_write(jbq_context *, jbq_value compiled, size_t *);
uint8_t *jbq_write_debug(jbq_context *, jbq_value compiled, size_t *);
jbq_value jbq_read(jbq_context *, const uint8_t *, size_t);
jbq_value jbq_eval_compiled(jbq_context *, jbq_value);
void jbq_buffer_free(jbq_context *, void *);
/* Snapshot includes string/symbol own keys in native order and enumerability. */
jbq_keys *jbq_own_keys(jbq_context *, jbq_value, size_t *);
jbq_value jbq_key(jbq_context *, const jbq_keys *, size_t);
int jbq_key_enumerable(const jbq_keys *, size_t);
void jbq_keys_free(jbq_context *, jbq_keys *);
int jbq_same_object(jbq_value, jbq_value);
int jbq_is_array(jbq_value);
int jbq_is_plain_record(jbq_context *, jbq_value);
/* 1 ordinary bytes, 0 unsupported, -1 detached/exception. Pointer borrowed until
 * next engine entry; Rust must copy immediately, before running JS/GC. */
int jbq_bytes_data(jbq_context *, jbq_value, const uint8_t **, size_t *);
jbq_value jbq_error(jbq_context *, jbq_value message);
/* Exact Error class only: own string data fields; if stack has no own
 * descriptor, read internal stored stack through a captured pristine native
 * getter. Own accessors and mutable prototypes are never invoked. */
/* field: 0 message, 1 stack, 2 name. */
jbq_value jbq_error_field(jbq_context *, jbq_value, int stack);
/* Prepared graph ABI: borrowed inputs, owned returned values; all native
 * module values must belong to this context. No resolution callbacks or eval
 * run during preparation/binding/linking. */
jbq_value jbq_module_prepare(jbq_context *,const uint16_t *,size_t,const char *);
int jbq_module_count(jbq_context *,jbq_value);
jbq_value jbq_module_import(jbq_context *,jbq_value,int);
int jbq_module_bind(jbq_context *,jbq_value,int,jbq_value);
int jbq_module_link(jbq_context *,jbq_value);
jbq_value jbq_module_namespace(jbq_context *,jbq_value);
int jbq_promise_state(jbq_context *,jbq_value);
jbq_value jbq_promise_result(jbq_context *,jbq_value);
uint8_t *jbq_module_write(jbq_context *,jbq_value,size_t *);
/* Local execution cache retains source; distribution writer above always strips it. */
uint8_t *jbq_module_cache_write(jbq_context *,jbq_value,size_t *);
jbq_value jbq_module_read(jbq_context *,const uint8_t *,size_t,jbq_value);
jbq_value jbq_module_builtin(jbq_context *,const char *,jbq_value);
/* Owner-local dynamic request queue, bounded at context construction. */
uint64_t jbq_import_take(jbq_context *,jbq_value *base,jbq_value *specifier);
int jbq_import_finish(jbq_context *,uint64_t,jbq_value module,jbq_value evaluation,const char *failure);

/* Compiler callback borrows source/lease/context synchronously, returns an owned
 * native compiled object or exception sentinel after pure Rust verification.
 * Never execute JS or unwind from the callback. No lease may escape it. */
typedef jbq_value (*jbq_compiler)(void *,jbq_context *,uint64_t,const uint16_t *,size_t,const char *,int,int);
int jbq_set_compiler(jbq_context *,jbq_compiler,void *);
/* lunarstorm patch: base compiler of a context the bridge does not own; see bridge.c. */
int jbq_set_base_compiler(void *js_context,jbq_compiler,void *);
int jbq_eval_environment(jbq_context *,uint64_t,JSJetbunnyEnvView *);
int jbq_eval_binding(jbq_context *,uint64_t,uint32_t,uint32_t,JSJetbunnyEnvBinding *);
jbq_value jbq_eval_binding_name(jbq_context *,uint64_t,uint32_t,uint32_t);
/* Unsafe FFI: safe Rust callers require verified immutable image types. */
jbq_value jbq_install_verified(jbq_context *,const uint8_t *,size_t,int prepare);
/* Throw a compile failure: 1 SyntaxError, 2 RangeError, else InternalError. */
jbq_value jbq_throw_compile(jbq_context *,int kind,const char *message);
int jbq_debug_eval_verified(jbq_context *,int,const uint16_t *,size_t,const char *,jbq_value *);
typedef JSJetbunnyLookupKey jbq_lookup_key;
int jbq_debug_lookup(jbq_context *,int,int,const uint16_t *,size_t,const jbq_lookup_key *,size_t,jbq_value *);
int jbq_debug_unwinding(jbq_context *);

/* Read-only post-install inspection. Handles borrow the owned compiled value;
 * no private engine layouts cross this ABI. All names/text are exact UTF16. */
const void *jbq_inspect_root(jbq_context *,jbq_value);
int jbq_inspect_view(jbq_context *,const void *,JSJetbunnyGraphView *);
int jbq_inspect_constant(jbq_context *,const void *,uint32_t,JSJetbunnyValueView *);
int jbq_inspect_local(jbq_context *,const void *,uint32_t,JSJetbunnyLocalView *);
int jbq_inspect_capture(jbq_context *,const void *,uint32_t,JSJetbunnyCaptureView *);
int jbq_inspect_text(jbq_context *,const void *,uint32_t,uint32_t,uint16_t *,size_t);
int jbq_inspect_bigint(jbq_context *,const void *,uint32_t,uint8_t *,size_t);
int jbq_inspect_module(jbq_context *,jbq_value,JSJetbunnyModuleView *);
int jbq_inspect_module_entry(jbq_context *,jbq_value,uint32_t,uint32_t,JSJetbunnyModuleEntry *);
/* Private native debugger substrate; owner thread only. No protocol policy.
 * Callback values are borrowed. Marker function tokens are valid only while
 * their compiled root is retained. No callback may unwind. */
typedef int (*jbq_debug_hook)(void *, jbq_context *, int);
typedef void (*jbq_debug_marker)(void *, uintptr_t, uint32_t, int, int, int);
typedef void (*jbq_debug_variable)(void *, jbq_value, jbq_value, int, int);
typedef struct {
 uintptr_t function;
 uint64_t activation;
 uint32_t marker;
 int line, column, scope_known;
} jbq_debug_frame;
void jbq_debug_install(jbq_context *, jbq_debug_hook, void *);

/* Owner-only, allocation-free continuation metadata. */
int jbq_debug_step_info(jbq_context *, uint64_t, uint64_t *, int *, uint64_t *);
typedef uint64_t (*jbq_debug_async_hook)(void *, jbq_context *, int, uint64_t);
void jbq_debug_async_install(jbq_context *, jbq_debug_async_hook, void *);
/* Borrowed native Promise/reason on the owner; hooks must retain before returning. */
typedef void (*jbq_debug_rejection_hook)(void *, jbq_context *, jbq_value, jbq_value, int, uint64_t);
void jbq_debug_rejection_install(jbq_context *, jbq_debug_rejection_hook, void *);
uintptr_t jbq_debug_promise_identity(jbq_context *, jbq_value);

uint64_t jbq_debug_async_capture(jbq_context *, int);
uint64_t jbq_debug_async_stack(jbq_context *);
uint64_t jbq_debug_async_set(jbq_context *, uint64_t);
int jbq_debug_frame_metadata(jbq_context *, int, jbq_debug_frame *);

void jbq_debug_flags(jbq_context *, int);
void jbq_debug_exception_policy(jbq_context *, int);
int jbq_debug_exception(jbq_context *, jbq_value *, int *);
int jbq_debug_frame_count(jbq_context *);
int jbq_debug_uninitialized(jbq_value);
int jbq_debug_markers(jbq_context *, jbq_value, jbq_debug_marker, void *);
int jbq_debug_breakpoint(jbq_context *, uintptr_t, uint32_t, int);
int jbq_debug_frame_at(jbq_context *, int, jbq_debug_frame *, jbq_value *, jbq_value *);
int jbq_debug_variables(jbq_context *, int, jbq_debug_variable, void *);
jbq_value jbq_debug_stop(jbq_context *);
/* Nonallocating take/reset; only explicit debugger stop sets this flag. */
int jbq_take_debug_stop(jbq_context *);
/* Only ordinary objects/arrays are expandable. Other classes stay opaque.
 * Enumeration has a 65536-key native bound checked BEFORE key allocation;
 * at most 1024 descriptors are materialized per call. Callback values borrow
 * the live descriptor. Returns 0 success, -2 capacity, -1 native/unsupported. */
/* Name is a string (raw description for symbols). Flags: bit 0 accessor,
 * bit 1 symbol key. No getters or symbol coercion execute during inspection. */
typedef void (*jbq_debug_property)(void *, jbq_context *, jbq_value, jbq_value, int);
int jbq_debug_object_class(jbq_context *, jbq_value);

typedef int (*jbq_debug_page_entry)(void *, jbq_context *, jbq_value, jbq_value, int, uint32_t, uint32_t, uint32_t);
typedef struct jbq_debug_value_info { char type_name[32]; uint16_t text[512]; size_t length; int expandable; } jbq_debug_value_info;
int jbq_debug_value_info_get(jbq_context *, jbq_value, jbq_debug_value_info *);
int jbq_debug_exception_description(jbq_context *, jbq_debug_value_info *);
typedef struct jbq_debug_cursor { size_t position,skip_remaining; uintptr_t collection_link; int phase,index,depth,last_level,skip_args; } jbq_debug_cursor;
void jbq_debug_cursor_retain(jbq_debug_cursor *);
void jbq_debug_cursor_release(jbq_context *, jbq_debug_cursor *);
int jbq_debug_inspect(jbq_context *, jbq_value, int, uint32_t, jbq_debug_cursor *, size_t, size_t,
                      jbq_debug_page_entry, void *, size_t *, int *);
int jbq_debug_copy_string(jbq_value, uint16_t *, size_t, size_t *);

int jbq_debug_properties(jbq_context *, jbq_value, size_t, size_t,
                         jbq_debug_property, void *, size_t *);
#endif

/* Owner-local exact edit target; capture returns NULL for read-only metadata. */
void *jbq_debug_target_new(jbq_context *,jbq_value,int,uint32_t,uint32_t,uint32_t,uint32_t,int *);
void jbq_debug_target_free(jbq_context *,void *);
int jbq_debug_target_validate(jbq_context *,void *);
int jbq_debug_target_store(jbq_context *,void *,jbq_value);

int jbq_debug_describe_exception(jbq_context *,jbq_value,jbq_debug_value_info *);
