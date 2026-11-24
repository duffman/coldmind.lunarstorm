/* Stable private ABI for Jetbunny. See bridge.h for ownership contract. */
#include "bridge.h"
#include "quickjs.h"
#include "jetbunny-modules.h"
#include <stdlib.h>
#include <string.h>
#include <limits.h>
_Static_assert(sizeof(JSValue) <= sizeof(jbq_value), "JSValue exceeds ABI container");
_Static_assert(sizeof(jbq_value) == 16, "value ABI changed");
typedef struct {
 uint64_t id;
 JSValue token,base,specifier;
 int taken;
} jbq_import;
static void dynamic_import(JSContext *,JSValueConst,JSValueConst,JSValue,JSValueConst,void *);
typedef struct { JSValue promise, reason; int handled; } jbq_rejection;
struct jbq_context {
 jbq_import imports[128]; uint64_t next_import;
 JSRuntime *rt; JSContext *ctx; JSClassID host_class, object_class, uint8_class;
 jbq_compiler compiler;void *compiler_opaque;
 jbq_debug_rejection_hook debug_rejection_hook;void *debug_rejection_opaque;
 jbq_debug_async_hook debug_async_hook;void *debug_async_opaque;
 jbq_debug_hook debug_hook; void *debug_opaque; int debug_enabled, debug_stopped;
 JSValue object_proto, error_stack_getter; jbq_interrupt interrupt; void *interrupt_opaque; void *entry;
 jbq_job_sink job_sink; void *job_opaque;
 /* Rejection-tracker events in arrival order; grown, never dropped. A failed growth
  * is recorded and reported by the next take instead of losing the event silently. */
 jbq_rejection *rejections; size_t rejection_head, rejection_count, rejection_capacity;
 int rejection_fault;
 /* Set once the platform has stored an async context; until then jobs skip the swap. */
 int async_context_used;
 /* lunarstorm patch: set by jbq_context_wrap; the runtime's handlers belong to
  * the context's owner and are not reset by jbq_context_free. */
 int borrowed;
};
struct jbq_keys { JSPropertyEnum *items; uint32_t length; };
typedef struct { jbq_host call; jbq_drop drop; void *opaque; } host_data;
static jbq_value pack(JSValue v) { jbq_value out={{0,0}}; memcpy(&out,&v,sizeof(v)); return out; }
static JSValue unpack(jbq_value v) { JSValue out; memcpy(&out,&v,sizeof(out)); return out; }
/* lunarstorm patch: the debugger half of `interrupted`, shared with the
 * owner's interrupt handler of a wrapped context (jbq_debug_poll). */
static int debug_poll(jbq_context *c) {
 int control=c->debug_hook?c->debug_hook(c->debug_opaque,c,-1):0;
 if(control==1)c->debug_stopped=1;
 return control;
}
static int interrupted(JSContext *ctx, void *opaque) {
 (void)ctx; jbq_context *c=opaque;
 if(c->interrupt && c->interrupt(c->interrupt_opaque)) return 1;
 return debug_poll(c);
}
int jbq_debug_poll(jbq_context *c){return c?debug_poll(c):0;}
static void host_finalizer(JSRuntime *rt, JSValue v) {
 (void)rt; host_data *h=JS_GetOpaque(v,JS_GetClassID(v));
 if(h) { /* Rust's drop trampoline contains panics and never enters JS. */
  if(h->drop) h->drop(h->opaque);
  free(h);
 }
}
/* Capture before application code runs. At the pinned revision the pristine
 * Error.prototype.stack getter only duplicates Error.[[ErrorData]]; it performs
 * no property lookup, coercion, user getter or other application execution.
 * Keep this native function rooted, independent of later prototype mutations. */
static int capture_error_stack_getter(jbq_context *c) {
 JSValue error=JS_NewError(c->ctx); if(JS_IsException(error)) return -1;
 JSValue proto=JS_GetPrototype(c->ctx,error); JS_FreeValue(c->ctx,error);
 if(JS_IsException(proto)) return -1;
 JSAtom atom=JS_NewAtom(c->ctx,"stack");
 if(!atom) { JS_FreeValue(c->ctx,proto); return -1; }
 JSPropertyDescriptor d; int found=JS_GetOwnProperty(c->ctx,&d,proto,atom);
 JS_FreeAtom(c->ctx,atom); JS_FreeValue(c->ctx,proto);
 if(found!=1) return -1;
 if((d.flags&JS_PROP_GETSET)&&JS_IsFunction(c->ctx,d.getter)) {
  c->error_stack_getter=JS_DupValue(c->ctx,d.getter);
 }
 JS_FreeValue(c->ctx,d.value); JS_FreeValue(c->ctx,d.getter); JS_FreeValue(c->ctx,d.setter);
 return JS_IsFunction(c->ctx,c->error_stack_getter)?0:-1;
}
/* The platform owns every non-ECMA-262 global. The context gets only ECMAScript
 * intrinsics: no AToB/DOMException and no performance object. queueMicrotask, which the
 * base objects install, is removed before any code runs. */
static JSContext *ecma_context(JSRuntime *rt) {
 JSContext *ctx=JS_NewContextRaw(rt); if(!ctx) return NULL;
 if(JS_AddIntrinsicBaseObjects(ctx)||JS_AddIntrinsicDate(ctx)||JS_AddIntrinsicEval(ctx)||
    JS_AddIntrinsicRegExp(ctx)||JS_AddIntrinsicJSON(ctx)||JS_AddIntrinsicProxy(ctx)||
    JS_AddIntrinsicMapSet(ctx)||JS_AddIntrinsicTypedArrays(ctx)||JS_AddIntrinsicPromise(ctx)||
    JS_AddIntrinsicWeakRef(ctx)) goto fail;
 JSValue global=JS_GetGlobalObject(ctx);
 JSAtom atom=JS_NewAtom(ctx,"queueMicrotask");
 int removed=atom?JS_DeleteProperty(ctx,global,atom,0):-1;
 if(atom) JS_FreeAtom(ctx,atom);
 JS_FreeValue(ctx,global);
 if(removed!=1) goto fail;
 return ctx;
fail:
 JS_FreeContext(ctx); return NULL;
}
static void track_rejection(JSContext *ctx,JSValueConst promise,JSValueConst reason,bool handled,void *opaque) {
 jbq_context *c=opaque;
 if(c->debug_rejection_hook) {
  uint64_t history=handled?0:JS_DebugPromiseRejectionOrigin(promise);
  c->debug_rejection_hook(c->debug_rejection_opaque,c,pack(promise),pack(reason),handled,history);
 }
 if(handled) {
  /* A handler attached before the host took the unhandled event cancels it: the
   * promise never stayed rejected without a handler across a drained microtask queue. */
  for(size_t i=0;i<c->rejection_count;i++) {
   jbq_rejection *r=&c->rejections[c->rejection_head+i];
   if(r->handled||JS_VALUE_GET_PTR(r->promise)!=JS_VALUE_GET_PTR(promise)) continue;
   JS_FreeValue(ctx,r->promise); JS_FreeValue(ctx,r->reason);
   memmove(r,r+1,(c->rejection_count-i-1)*sizeof(*r));
   if(!--c->rejection_count) c->rejection_head=0;
   return;
  }
 }
 /* The public raw tracker is independent of debugger consumption. Capacity loss
  * is explicit when the application takes events, never an application throw. */
 if(c->rejection_count==1024) {c->rejection_fault=2;return;}
 if(c->rejection_head+c->rejection_count==c->rejection_capacity) {
  if(c->rejection_head) {
   memmove(c->rejections,c->rejections+c->rejection_head,c->rejection_count*sizeof(*c->rejections));
   c->rejection_head=0;
  } else {
   size_t capacity=c->rejection_capacity?c->rejection_capacity*2:16;
   jbq_rejection *grown=capacity>SIZE_MAX/sizeof(*grown)?NULL:
    js_realloc_rt(c->rt,c->rejections,capacity*sizeof(*grown));
   if(!grown) { c->rejection_fault=1; return; }
   c->rejections=grown; c->rejection_capacity=capacity;
  }
 }
 jbq_rejection *r=&c->rejections[c->rejection_head+c->rejection_count++];
 r->promise=JS_DupValue(ctx,promise); r->reason=JS_DupValue(ctx,reason); r->handled=handled;
}
static void free_rejections(jbq_context *c) {
 for(size_t i=0;i<c->rejection_count;i++) {
  jbq_rejection *r=&c->rejections[c->rejection_head+i];
  JS_FreeValue(c->ctx,r->promise); JS_FreeValue(c->ctx,r->reason);
 }
 js_free_rt(c->rt,c->rejections);
 c->rejections=NULL; c->rejection_head=c->rejection_count=c->rejection_capacity=0;
}
/**
 * Creates the bridge context on a runtime the caller owns (rquickjs `Runtime`).
 *
 * Decision: the runtime is borrowed, never freed here, so rquickjs and the bridge share one
 * heap. Class IDs are allocated per runtime in this pin and rquickjs claims the first ones
 * for its own classes, so the host class is allocated on this runtime, per context; the
 * process-wide ID taken from a throwaway runtime (jbq_init_host_class) collided with them.
 *
 * @param rt Runtime that outlives the context.
 * @param memory Heap limit set on the runtime.
 * @param stack Native stack limit set on the runtime.
 * @param interrupt Interrupt poll; NULL for none.
 * @param opaque Argument of interrupt.
 * @return The context, or NULL on allocation or class registration failure.
 */
jbq_context *jbq_context_new(void *rt, size_t memory, size_t stack,
                            jbq_interrupt interrupt, void *opaque) {
 if(!rt) return NULL;
 jbq_context *c=calloc(1,sizeof(*c)); if(!c) return NULL;
 c->object_proto=JS_UNDEFINED; c->error_stack_getter=JS_UNDEFINED; c->interrupt=interrupt; c->interrupt_opaque=opaque;
 c->rt=rt;
 JS_SetMemoryLimit(c->rt,memory); JS_SetMaxStackSize(c->rt,stack);
 JSClassID host_class=0; JS_NewClassID(c->rt,&host_class);
 c->host_class=host_class;
 JSClassDef def={0}; def.class_name="JetbunnyHostData"; def.finalizer=host_finalizer;
 if(JS_NewClass(c->rt,host_class,&def)<0) goto fail;
 c->ctx=ecma_context(c->rt); if(!c->ctx) goto fail;
 JS_SetContextOpaque(c->ctx,c);
 JSValue object=JS_NewObject(c->ctx);
 if(JS_IsException(object)) goto fail;
 c->object_class=JS_GetClassID(object); c->object_proto=JS_GetPrototype(c->ctx,object);
 JS_FreeValue(c->ctx,object);
 if(JS_IsException(c->object_proto)) goto fail;
 JSValue bytes=JS_NewUint8ArrayCopy(c->ctx,NULL,0);
 if(JS_IsException(bytes)) goto fail;
 c->uint8_class=JS_GetClassID(bytes); JS_FreeValue(c->ctx,bytes);
 if(capture_error_stack_getter(c)<0) goto fail;
 JS_SetInterruptHandler(c->rt,interrupted,c); JS_JetbunnySetDynamicImportFunc(c->rt,dynamic_import,c);
 JS_SetHostPromiseRejectionTracker(c->rt,track_rejection,c);
 return c;
fail:
 if(c->ctx) { JS_FreeValue(c->ctx,c->error_stack_getter); JS_FreeValue(c->ctx,c->object_proto); JS_FreeContext(c->ctx); }
 free(c); return NULL;
}
/**
 * lunarstorm patch: the bridge around a context rquickjs created, for the debugger.
 *
 * Decision: everything jbq_context_new sets on the runtime (limits, interrupt
 * handler, rejection tracker, dynamic import) stays the owner's; only the
 * per-context state the debugger and value helpers read is built: the host
 * class, Object.prototype, the Uint8Array class and the pristine stack getter.
 * The context opaque is taken; rquickjs does not use it.
 *
 * @param js_context A live JSContext; one reference is taken.
 * @return The bridge context, or NULL on allocation or class registration failure.
 */
jbq_context *jbq_context_wrap(void *js_context) {
 JSContext *ctx=js_context; if(!ctx) return NULL;
 jbq_context *c=calloc(1,sizeof(*c)); if(!c) return NULL;
 c->object_proto=JS_UNDEFINED; c->error_stack_getter=JS_UNDEFINED; c->borrowed=1;
 c->rt=JS_GetRuntime(ctx);
 JSClassID host_class=0; JS_NewClassID(c->rt,&host_class);
 c->host_class=host_class;
 JSClassDef def={0}; def.class_name="JetbunnyHostData"; def.finalizer=host_finalizer;
 if(JS_NewClass(c->rt,host_class,&def)<0) { free(c); return NULL; }
 c->ctx=JS_DupContext(ctx);
 JS_SetContextOpaque(c->ctx,c);
 JSValue object=JS_NewObject(c->ctx);
 if(JS_IsException(object)) goto fail;
 c->object_class=JS_GetClassID(object); c->object_proto=JS_GetPrototype(c->ctx,object);
 JS_FreeValue(c->ctx,object);
 if(JS_IsException(c->object_proto)) goto fail;
 JSValue bytes=JS_NewUint8ArrayCopy(c->ctx,NULL,0);
 if(JS_IsException(bytes)) goto fail;
 c->uint8_class=JS_GetClassID(bytes); JS_FreeValue(c->ctx,bytes);
 if(capture_error_stack_getter(c)<0) goto fail;
 return c;
fail:
 JS_SetContextOpaque(c->ctx,NULL);
 JS_FreeValue(c->ctx,c->error_stack_getter); JS_FreeValue(c->ctx,c->object_proto); JS_FreeContext(c->ctx);
 free(c); return NULL;
}
/**
 * Releases the bridge's context reference and state; the runtime stays with its owner.
 *
 * @param c Bridge context; NULL is ignored.
 */
void jbq_context_free(jbq_context *c) { if(c) {
 /* The host has released its queued jobs; jobs created from here on (dynamic-import
  * rejections, finalizers) stay in the runtime's list, which the runtime's owner frees. */
 jbq_set_job_sink(c,NULL,NULL);
 JS_JetbunnySetCompiler(c->ctx,NULL,NULL); for(size_t i=0;i<128;i++) if(c->imports[i].id) {
 JSValue error=JS_NewString(c->ctx,"runtime closed during dynamic import");
 if(JS_IsException(error))error=JS_GetException(c->ctx);
 JS_FinishDynamicImport(c->ctx,c->imports[i].token,JS_EXCEPTION,error);
 JS_FreeValue(c->ctx,c->imports[i].base);JS_FreeValue(c->ctx,c->imports[i].specifier);
 c->imports[i].id=0;
 } }

 /* Values held outside the heap are released before the context frees its objects. */
 if(!c->borrowed) { /* lunarstorm patch: a wrapped context's runtime handlers are its owner's */
  JS_SetHostPromiseRejectionTracker(c->rt,NULL,NULL); JS_SetInterruptHandler(c->rt,NULL,NULL);
  JS_JetbunnySetDynamicImportFunc(c->rt,NULL,NULL);
 } else {
  JS_SetDebugHook(c->ctx,NULL,NULL); JS_SetContextOpaque(c->ctx,NULL);
 }
 free_rejections(c);
 JS_JetbunnySetAsyncContext(c->ctx,JS_UNDEFINED);
 JS_FreeValue(c->ctx,c->error_stack_getter);
 JS_FreeValue(c->ctx,c->object_proto); JS_FreeContext(c->ctx);
 free(c);
}
/**
 * The engine context under the bridge, for a second owner (rquickjs `Context`).
 *
 * @param c Bridge context.
 * @return The context with one new reference the caller releases with JS_FreeContext.
 */
void *jbq_js_context(jbq_context *c) { return JS_DupContext(c->ctx); }
void jbq_set_entry(jbq_context *c,void *entry) { c->entry=entry; }
void *jbq_get_entry(jbq_context *c) { return c->entry; }
void jbq_gc(jbq_context *c) { JS_RunGC(c->rt); }
/**
 * Reports the runtime heap's size and ceiling for the host.
 *
 * Decision: `malloc_size` (allocated bytes including allocator overhead) is the figure the
 * memory limit is checked against, so the two numbers compare directly.
 *
 * @param c Bridge context.
 * @param used Receives the bytes the runtime has allocated now.
 * @param limit Receives the memory limit; 0 when the runtime has none.
 */
void jbq_heap_usage(jbq_context *c,int64_t *used,int64_t *limit) {
 JSMemoryUsage u; JS_ComputeMemoryUsage(c->rt,&u); *used=u.malloc_size; *limit=u.malloc_limit;
}
int jbq_take_oom(jbq_context *c) { return JS_JetbunnyTakeOutOfMemory(c->rt); }
/**
 * The address of the runtime's sticky out-of-memory latch.
 * @param c The context.
 * @return One byte, nonzero while an allocation failure is latched; valid until
 *         jbq_context_free. Read only; jbq_take_oom takes and clears it.
 */
const unsigned char *jbq_oom_flag(jbq_context *c) { return (const unsigned char *)JS_JetbunnyOutOfMemoryFlag(c->rt); }
void jbq_set_memory_limit(jbq_context *c,size_t limit) { JS_SetMemoryLimit(c->rt,limit); }
const char *jbq_version(void) { return JS_GetVersion(); }
jbq_value jbq_dup(jbq_context *c,jbq_value v) { return pack(JS_DupValue(c->ctx,unpack(v))); }
void jbq_free(jbq_context *c,jbq_value v) { JS_FreeValue(c->ctx,unpack(v)); }
jbq_value jbq_global(jbq_context *c) { return pack(JS_GetGlobalObject(c->ctx)); }
jbq_value jbq_undefined(void) { return pack(JS_UNDEFINED); }
jbq_value jbq_null(void) { return pack(JS_NULL); }
jbq_value jbq_bool(jbq_context *c,int v) { return pack(JS_NewBool(c->ctx,v!=0)); }
jbq_value jbq_number(jbq_context *c,double v) { return pack(JS_NewFloat64(c->ctx,v)); }
jbq_value jbq_string(jbq_context *c,const uint16_t *v,size_t n) { return pack(JS_NewStringUTF16(c->ctx,v,n)); }
jbq_value jbq_bytes(jbq_context *c,const uint8_t *v,size_t n) { return pack(JS_NewUint8ArrayCopy(c->ctx,v,n)); }
jbq_value jbq_array(jbq_context *c) { return pack(JS_NewArray(c->ctx)); }
jbq_value jbq_record(jbq_context *c) { return pack(JS_NewObjectProto(c->ctx,JS_NULL)); }
int jbq_kind(jbq_context *c,jbq_value value) {
 JSValue v=unpack(value);
 if(JS_IsUndefined(v)) return 0;
 if(JS_IsNull(v)) return 1;
 if(JS_IsBool(v)) return 2;
 if(JS_IsNumber(v)) return 3;
 if(JS_IsString(v)) return 4;
 if(JS_IsSymbol(v)) return 5;
 if(JS_IsBigInt(v)) return 6;
 if(JS_IsFunction(c->ctx,v)) return 8;
 return 7;
}
int jbq_is_exception(jbq_value v) { return JS_IsException(unpack(v)); }
int jbq_is_uncatchable(jbq_value v) { return JS_IsUncatchableError(unpack(v)); }
jbq_value jbq_take_exception(jbq_context *c) { return pack(JS_GetException(c->ctx)); }
int jbq_to_bool(jbq_context *c,jbq_value v) { return JS_ToBool(c->ctx,unpack(v)); }
int jbq_to_number(jbq_context *c,jbq_value v,double *out) { return JS_ToFloat64(c->ctx,out,unpack(v)); }
const uint16_t *jbq_string_data(jbq_context *c,jbq_value v,size_t *n) {
 if(!JS_IsString(unpack(v))) { JS_ThrowTypeError(c->ctx,"expected primitive string"); return NULL; }
 return JS_ToCStringLenUTF16(c->ctx,n,unpack(v));
}
void jbq_string_free(jbq_context *c,const uint16_t *v) { JS_FreeCStringUTF16(c->ctx,v); }
/* Taken from rquickjs (String::to_string): one copy straight to UTF-8 instead of UTF-16 and
 * a transcode in Rust. Decision: cesu8=0, so a pair becomes one 4-byte sequence and an
 * unpaired surrogate stays a WTF-8 sequence the Rust side rejects as UTF-8. */
const char *jbq_string_utf8(jbq_context *c,jbq_value v,size_t *n) {
 if(!JS_IsString(unpack(v))) { JS_ThrowTypeError(c->ctx,"expected primitive string"); return NULL; }
 return JS_ToCStringLen2(c->ctx,n,unpack(v),0);
}
void jbq_utf8_free(jbq_context *c,const char *v) { JS_FreeCString(c->ctx,v); }
jbq_value jbq_string_from_utf8(jbq_context *c,const char *v,size_t n) { return pack(JS_NewStringLen(c->ctx,v,n)); }
/* Taken from rquickjs (predefined atoms): a property name resolved to an atom once and
 * reused. Decision: built from UTF-16, so names with unpaired surrogates stay lossless. */
uint32_t jbq_atom(jbq_context *c,const uint16_t *v,size_t n) {
 JSValue s=JS_NewStringUTF16(c->ctx,v,n); if(JS_IsException(s)) return JS_ATOM_NULL;
 JSAtom a=JS_ValueToAtom(c->ctx,s); JS_FreeValue(c->ctx,s); return a;
}
void jbq_atom_free(jbq_context *c,uint32_t a) { JS_FreeAtom(c->ctx,a); }
jbq_value jbq_get_atom(jbq_context *c,jbq_value object,uint32_t a) { return pack(JS_GetProperty(c->ctx,unpack(object),a)); }
int jbq_set_atom(jbq_context *c,jbq_value object,uint32_t a,jbq_value value) {
 return JS_SetProperty(c->ctx,unpack(object),a,JS_DupValue(c->ctx,unpack(value)));
}
static JSAtom key_atom(jbq_context *c,jbq_value key) {
 JSValue k=unpack(key); if(!JS_IsString(k)&&!JS_IsSymbol(k)) {
  JS_ThrowTypeError(c->ctx,"expected primitive property key"); return JS_ATOM_NULL;
 }
 return JS_ValueToAtom(c->ctx,k);
}
jbq_value jbq_get(jbq_context *c,jbq_value object,jbq_value key) {
 JSAtom a=key_atom(c,key); if(!a) return pack(JS_EXCEPTION);
 JSValue v=JS_GetProperty(c->ctx,unpack(object),a); JS_FreeAtom(c->ctx,a); return pack(v);
}
int jbq_set(jbq_context *c,jbq_value object,jbq_value key,jbq_value value) {
 JSAtom a=key_atom(c,key); if(!a) return -1;
 int r=JS_SetProperty(c->ctx,unpack(object),a,JS_DupValue(c->ctx,unpack(value)));
 JS_FreeAtom(c->ctx,a); return r;
}
jbq_value jbq_get_index(jbq_context *c,jbq_value v,uint32_t i) { return pack(JS_GetPropertyUint32(c->ctx,unpack(v),i)); }
int jbq_define_index(jbq_context *c,jbq_value v,uint32_t i,jbq_value x) {
 return JS_DefinePropertyValueUint32(c->ctx,unpack(v),i,JS_DupValue(c->ctx,unpack(x)),JS_PROP_C_W_E|JS_PROP_THROW);
}
int jbq_has_own_index(jbq_context *c,jbq_value v,uint32_t i) {
 JSAtom a=JS_NewAtomUInt32(c->ctx,i); if(!a) return -1;
 int r=JS_GetOwnProperty(c->ctx,NULL,unpack(v),a); JS_FreeAtom(c->ctx,a); return r;
}
int jbq_length(jbq_context *c,jbq_value v,int64_t *n) { return JS_GetLength(c->ctx,unpack(v),n); }
int jbq_define_key(jbq_context *c,jbq_value v,jbq_value key,jbq_value x) {
 JSAtom a=key_atom(c,key); if(!a) return -1;
 int r=JS_DefinePropertyValue(c->ctx,unpack(v),a,JS_DupValue(c->ctx,unpack(x)),JS_PROP_C_W_E|JS_PROP_THROW);
 JS_FreeAtom(c->ctx,a); return r;
}
/* Fix: every call from the host allocated its argument vector on the engine heap and freed
 * it again. Decision: up to JBQ_STACK_ARGS arguments are unpacked into the caller's stack
 * array; only longer lists take the heap. */
#define JBQ_STACK_ARGS 8
/**
 * The native argument vector of a host call.
 * @param c The context.
 * @param args The packed arguments.
 * @param n Their count.
 * @param stack The caller's array of JBQ_STACK_ARGS values, used when n fits.
 * @return The vector (stack, or a js_malloc block the caller frees with arguments_free);
 *         NULL when n is 0, or with a pending exception when n is not 0.
 */
static JSValue *arguments(jbq_context *c,const jbq_value *args,size_t n,JSValue *stack) {
 if(n>(size_t)INT_MAX||n>SIZE_MAX/sizeof(JSValue)) { JS_ThrowRangeError(c->ctx,"argument count overflow"); return NULL; }
 if(!n) return NULL;
 JSValue *out=stack;
 if(n>JBQ_STACK_ARGS) { out=js_malloc(c->ctx,n*sizeof(*out)); if(!out) { JS_ThrowOutOfMemory(c->ctx); return NULL; } }
 for(size_t i=0;i<n;i++) out[i]=unpack(args[i]);
 return out;
}
/**
 * Releases a vector of arguments(); the stack array is left alone.
 * @param c The context.
 * @param a The vector, or NULL.
 * @param stack The stack array passed to arguments().
 */
static void arguments_free(jbq_context *c,JSValue *a,JSValue *stack) { if(a!=stack) js_free(c->ctx,a); }
jbq_value jbq_call(jbq_context *c,jbq_value f,jbq_value receiver,const jbq_value *args,size_t n) {
 JSValue stack[JBQ_STACK_ARGS]; JSValue *a=arguments(c,args,n,stack); if(n&&!a) return pack(JS_EXCEPTION);
 JSValue r=JS_Call(c->ctx,unpack(f),unpack(receiver),(int)n,a); arguments_free(c,a,stack); return pack(r);
}
jbq_value jbq_construct(jbq_context *c,jbq_value f,jbq_value target,const jbq_value *args,size_t n) {
 JSValue stack[JBQ_STACK_ARGS]; JSValue *a=arguments(c,args,n,stack); if(n&&!a) return pack(JS_EXCEPTION);
 JSValue r=JS_CallConstructor2(c->ctx,unpack(f),unpack(target),(int)n,a); arguments_free(c,a,stack); return pack(r);
}
static JSValue host_call(JSContext *ctx,JSValueConst receiver,int argc,JSValueConst *argv,int magic,JSValueConst *data) {
 (void)magic; jbq_context *c=JS_GetContextOpaque(ctx);
 if(JS_JetbunnyDebugPoll(ctx))return JS_EXCEPTION;
 host_data *h=JS_GetOpaque(data[0],c->host_class); if(!h) return JS_ThrowInternalError(ctx,"missing host registration");
 int thrown=0; JSValue v=unpack(h->call(h->opaque,c,pack(receiver),argv,(size_t)argc,&thrown));
 if(JS_JetbunnyDebugPoll(ctx)){JS_FreeValue(ctx,v);return JS_EXCEPTION;}
 if(thrown==1) { JS_DebugNewThrow(ctx); return JS_Throw(ctx,v); }
 if(thrown) { JS_FreeValue(ctx,v); return JS_ThrowInternalError(ctx,"host callback fault"); }
 return v;
}
jbq_value jbq_host_function(jbq_context *c,jbq_value name,uint16_t arity,jbq_host callback,void *opaque,jbq_drop drop) {
 host_data *h=malloc(sizeof(*h)); if(!h) { if(drop) drop(opaque); return pack(JS_ThrowOutOfMemory(c->ctx)); }
 *h=(host_data){callback,drop,opaque};
 JSValue holder=JS_NewObjectClass(c->ctx,c->host_class);
 if(JS_IsException(holder)) { if(drop) drop(opaque); free(h); return pack(holder); }
 if(JS_SetOpaque(holder,h)<0) { JS_FreeValue(c->ctx,holder); if(drop) drop(opaque); free(h); return pack(JS_ThrowInternalError(c->ctx,"host registration failed")); }
 JSValue f=JS_NewCFunctionData(c->ctx,host_call,arity,0,1,&holder); JS_FreeValue(c->ctx,holder);
 if(JS_IsException(f)) return pack(f);
 if(JS_DefinePropertyValueStr(c->ctx,f,"name",JS_DupValue(c->ctx,unpack(name)),JS_PROP_CONFIGURABLE|JS_PROP_THROW)<0) {
  JS_FreeValue(c->ctx,f); return pack(JS_EXCEPTION);
 }
 return pack(f);
}
jbq_value jbq_argv_get(const void *argv,size_t i) { return pack(((const JSValue *)argv)[i]); }
jbq_value jbq_deferred(jbq_context *c,jbq_value *pair) {
 *pair=pack(JS_UNDEFINED); JSValue funcs[2]; JSValue p=JS_NewPromiseCapability(c->ctx,funcs);
 if(JS_IsException(p)) return pack(p);
 JSValue o=JS_NewObjectProto(c->ctx,JS_NULL);
 if(JS_IsException(o)) { JS_FreeValue(c->ctx,funcs[0]); JS_FreeValue(c->ctx,funcs[1]); JS_FreeValue(c->ctx,p); return pack(o); }
 if(JS_DefinePropertyValueUint32(c->ctx,o,0,funcs[0],JS_PROP_C_W_E)<0) {
  JS_FreeValue(c->ctx,funcs[1]); JS_FreeValue(c->ctx,o); JS_FreeValue(c->ctx,p); return pack(JS_EXCEPTION);
 }
 if(JS_DefinePropertyValueUint32(c->ctx,o,1,funcs[1],JS_PROP_C_W_E)<0) {
  JS_FreeValue(c->ctx,o); JS_FreeValue(c->ctx,p); return pack(JS_EXCEPTION);
 }
 *pair=pack(o); return pack(p);
}
jbq_value jbq_settle(jbq_context *c,jbq_value pair,int reject,jbq_value value) {
 JSValue f=JS_GetPropertyUint32(c->ctx,unpack(pair),reject?1:0);
 if(JS_IsException(f)) return pack(f);
 JSValue arg=unpack(value); JSValue r=JS_Call(c->ctx,f,JS_UNDEFINED,1,&arg); JS_FreeValue(c->ctx,f); return pack(r);
}
int jbq_job_pending(jbq_context *c) { return JS_IsJobPending(c->rt); }
int jbq_execute_job(jbq_context *c) { JSContext *ctx=NULL; return JS_ExecutePendingJob(c->rt,&ctx); }
static int job_sink(void *opaque,JSJetbunnyJob *job) {
 jbq_context *c=opaque; return c->job_sink(c->job_opaque,(jbq_job *)job);
}
void jbq_set_job_sink(jbq_context *c,jbq_job_sink sink,void *opaque) {
 c->job_sink=sink; c->job_opaque=opaque;
 JS_JetbunnySetJobSink(c->rt,sink?job_sink:NULL,sink?c:NULL);
}
int jbq_run_job(jbq_context *c,jbq_job *job) { return JS_JetbunnyRunJob(c->rt,(JSJetbunnyJob *)job)<0?-1:1; }
void jbq_drop_job(jbq_context *c,jbq_job *job) { JS_JetbunnyFreeJob(c->rt,(JSJetbunnyJob *)job); }
/* argv: the callback and the async context captured when it was queued. */
static JSValue microtask_job(JSContext *ctx,int argc,JSValueConst *argv) {
 (void)argc; jbq_context *c=JS_GetContextOpaque(ctx);
 if(!c->async_context_used) return JS_Call(ctx,argv[0],JS_UNDEFINED,0,NULL);
 JSValue previous=JS_JetbunnyGetAsyncContext(ctx);
 JS_JetbunnySetAsyncContext(ctx,argv[1]);
 JSValue result=JS_Call(ctx,argv[0],JS_UNDEFINED,0,NULL);
 JS_JetbunnySetAsyncContext(ctx,previous); JS_FreeValue(ctx,previous);
 return result;
}
int jbq_enqueue_job(jbq_context *c,jbq_value function) {
 JSValue f=unpack(function);
 if(!JS_IsFunction(c->ctx,f)) { JS_ThrowTypeError(c->ctx,"microtask callback is not a function"); return -1; }
 JSValue args[2]={f,JS_JetbunnyGetAsyncContext(c->ctx)};
 uint64_t previous=JS_DebugSetAsyncStack(c->ctx,JS_DebugCaptureAsync(c->ctx,JS_DEBUG_ASYNC_MICROTASK));
 int status=JS_EnqueueJob(c->ctx,microtask_job,2,args);
 JS_DebugSetAsyncStack(c->ctx,previous);
 JS_FreeValue(c->ctx,args[1]);
 if(status<0&&!JS_HasException(c->ctx)) JS_ThrowOutOfMemory(c->ctx);
 return status<0?-1:0;
}
jbq_value jbq_async_context(jbq_context *c) { return pack(JS_JetbunnyGetAsyncContext(c->ctx)); }
void jbq_set_async_context(jbq_context *c,jbq_value value) {
 c->async_context_used=1; JS_JetbunnySetAsyncContext(c->ctx,unpack(value));
}
int jbq_take_rejection(jbq_context *c,jbq_value *promise,jbq_value *reason) {
 *promise=pack(JS_UNDEFINED); *reason=pack(JS_UNDEFINED);
 if(c->rejection_fault) { int fault=c->rejection_fault;c->rejection_fault=0;
  if(fault==2)return -2;
  JS_ThrowOutOfMemory(c->ctx); return -1; }
 if(!c->rejection_count) return 0;
 jbq_rejection r=c->rejections[c->rejection_head++]; c->rejection_count--;
 if(!c->rejection_count) c->rejection_head=0;
 *promise=pack(r.promise); *reason=pack(r.reason); return r.handled?2:1;
}
int jbq_detach_buffer(jbq_context *c,jbq_value value) {
 JSValue v=unpack(value);
 if(!JS_IsArrayBuffer(v)) { JS_ThrowTypeError(c->ctx,"expected an ArrayBuffer"); return -1; }
 JS_DetachArrayBuffer(c->ctx,v); return 0;
}
/* 0 none; 1 pending promise; 2 fulfilled and 3 rejected promise (first = result); 4 proxy
 * (first = target, second = handler, both null once revoked); -1 with an exception. */
int jbq_internals(jbq_context *c,jbq_value value,jbq_value *first,jbq_value *second) {
 JSValue v=unpack(value); *first=pack(JS_UNDEFINED); *second=pack(JS_UNDEFINED);
 if(JS_IsPromise(v)) {
  JSPromiseStateEnum s=JS_PromiseState(c->ctx,v);
  if(s==JS_PROMISE_PENDING) return 1;
  JSValue r=JS_PromiseResult(c->ctx,v); if(JS_IsException(r)) return -1;
  *first=pack(r); return s==JS_PROMISE_FULFILLED?2:3;
 }
 if(JS_IsProxy(v)) {
  /* The only failure for a proxy is revocation, which reads as null target and handler. */
  JSValue t=JS_GetProxyTarget(c->ctx,v);
  if(JS_IsException(t)) { JS_FreeValue(c->ctx,JS_GetException(c->ctx)); *first=pack(JS_NULL); *second=pack(JS_NULL); return 4; }
  JSValue h=JS_GetProxyHandler(c->ctx,v);
  *first=pack(t); *second=pack(h); return 4;
 }
 return 0;
}
static jbq_value source(jbq_context *c,const uint16_t *text,size_t n,const char *origin,int compile) {
 if(n>(SIZE_MAX-1)/3) return pack(JS_ThrowRangeError(c->ctx,"source size overflow"));
 char *buf=js_malloc(c->ctx,n*3+1); if(!buf) return pack(JS_ThrowOutOfMemory(c->ctx));
 size_t length=0;
 /* WTF-8: paired code units are scalar UTF-8 for identifier lexing, while
  * isolated surrogates retain their exact code units in string literals.
  * Four output bytes per pair still fit the three-bytes-per-unit bound. */
 for(size_t i=0;i<n;i++) { uint32_t u=text[i];
  if(u>=0xd800 && u<=0xdbff && i+1<n && text[i+1]>=0xdc00 && text[i+1]<=0xdfff) {
   u=0x10000+((u-0xd800)<<10)+(text[++i]-0xdc00);
  }
  if(u<0x80) buf[length++]=(char)u;
  else if(u<0x800) { buf[length++]=(char)(0xc0|(u>>6)); buf[length++]=(char)(0x80|(u&63)); }
  else if(u<0x10000) { buf[length++]=(char)(0xe0|(u>>12)); buf[length++]=(char)(0x80|((u>>6)&63)); buf[length++]=(char)(0x80|(u&63)); }
  else { buf[length++]=(char)(0xf0|(u>>18)); buf[length++]=(char)(0x80|((u>>12)&63));
   buf[length++]=(char)(0x80|((u>>6)&63)); buf[length++]=(char)(0x80|(u&63)); }
 }
 buf[length]=0;
 /* lunarstorm patch: with the C parser restored, no compiler callback adds
  * JS_EVAL_FLAG_DEBUG while a debugger is installed (jetbunny's
  * compiler_callback did: debug_enabled && !lease). Without it the C parser
  * emits no `debugger` pause and leaves locals uncaptured, so a debugger
  * evaluation wrote to a detached copy (`x = 99` in a paused block scope was
  * lost). Same rule here: every compile under an installed hook is a debug
  * compile. */
 int debug=compile==3||c->debug_enabled;
 JSValue r=JS_Eval(c->ctx,buf,length,origin,(compile==2?JS_EVAL_TYPE_MODULE:JS_EVAL_TYPE_GLOBAL)|(compile?JS_EVAL_FLAG_COMPILE_ONLY:0)|(compile==2?JS_EVAL_FLAG_JETBUNNY_PREPARE:0)|(debug?JS_EVAL_FLAG_DEBUG:0));
 js_free(c->ctx,buf); return pack(r);
}
jbq_value jbq_eval(jbq_context *c,const uint16_t *s,size_t n,const char *origin) { return source(c,s,n,origin,0); }
jbq_value jbq_compile(jbq_context *c,const uint16_t *s,size_t n,const char *origin) { return source(c,s,n,origin,1); }
uint8_t *jbq_write(jbq_context *c,jbq_value v,size_t *n) { return JS_WriteObject(c->ctx,n,unpack(v),JS_WRITE_OBJ_BYTECODE|JS_WRITE_OBJ_STRIP_SOURCE|JS_WRITE_OBJ_STRIP_DEBUG); }
jbq_value jbq_read(jbq_context *c,const uint8_t *p,size_t n) { return pack(JS_ReadObject(c->ctx,p,n,JS_READ_OBJ_BYTECODE)); }
jbq_value jbq_eval_compiled(jbq_context *c,jbq_value v) {
 JSValue r=JS_EvalFunction(c->ctx,JS_DupValue(c->ctx,unpack(v)));
 /* The host reads a module's evaluation promise itself: its rejection is the module's
  * failure, never an unhandled promise rejection. */
 if(JS_IsPromise(r)) JS_PromiseMarkAsHandled(c->ctx,r);
 return pack(r);
}
void jbq_buffer_free(jbq_context *c,void *p) { js_free(c->ctx,p); }
jbq_keys *jbq_own_keys(jbq_context *c,jbq_value v,size_t *n) {
 *n=0; jbq_keys *k=js_malloc(c->ctx,sizeof(*k)); if(!k) { JS_ThrowOutOfMemory(c->ctx); return NULL; }
 if(JS_GetOwnPropertyNames(c->ctx,&k->items,&k->length,unpack(v),JS_GPN_STRING_MASK|JS_GPN_SYMBOL_MASK|JS_GPN_SET_ENUM)<0) { js_free(c->ctx,k); return NULL; }
 *n=k->length; return k;
}
jbq_value jbq_key(jbq_context *c,const jbq_keys *k,size_t i) { return pack(JS_AtomToValue(c->ctx,k->items[i].atom)); }
int jbq_key_enumerable(const jbq_keys *k,size_t i) { return k->items[i].is_enumerable; }
void jbq_keys_free(jbq_context *c,jbq_keys *k) { if(k) { JS_FreePropertyEnum(c->ctx,k->items,k->length); js_free(c->ctx,k); } }
int jbq_same_object(jbq_value a,jbq_value b) {
 JSValue x=unpack(a),y=unpack(b); return JS_IsObject(x)&&JS_IsObject(y)&&JS_VALUE_GET_PTR(x)==JS_VALUE_GET_PTR(y);
}
int jbq_is_array(jbq_value v) { return JS_IsArray(unpack(v)); }
int jbq_is_plain_record(jbq_context *c,jbq_value v) {
 JSValue x=unpack(v); if(JS_GetClassID(x)!=c->object_class) return 0;
 JSValue p=JS_GetPrototype(c->ctx,x);
 if(JS_IsException(p)) return -1;
 int r=JS_IsNull(p)||JS_IsStrictEqual(c->ctx,p,c->object_proto); JS_FreeValue(c->ctx,p); return r;
}
int jbq_bytes_data(jbq_context *c,jbq_value v,const uint8_t **out,size_t *n) {
 *out=NULL; *n=0; JSValue x=unpack(v);
 if(JS_IsArrayBuffer(x)) { *out=JS_GetArrayBuffer(c->ctx,n,x); return JS_HasException(c->ctx)?-1:1; }
 if(JS_GetClassID(x)!=c->uint8_class) return 0;
 size_t offset,length,total; JSValue buffer=JS_GetTypedArrayBuffer(c->ctx,x,&offset,&length,NULL);
 if(JS_IsException(buffer)) return -1;
 if(!JS_IsArrayBuffer(buffer)) { JS_FreeValue(c->ctx,buffer); return 0; }
 const uint8_t *data=JS_GetArrayBuffer(c->ctx,&total,buffer);
 int r=JS_HasException(c->ctx)?-1:1;
 if(r==1) { if(offset>total||length>total-offset) { JS_ThrowTypeError(c->ctx,"invalid byte view"); r=-1; }
  else { *out=data?data+offset:NULL; *n=length; }
 }
 JS_FreeValue(c->ctx,buffer); return r;
}
jbq_value jbq_error(jbq_context *c,jbq_value message) {
 JSValue v=JS_NewError(c->ctx);
 if(JS_IsException(v)) return pack(v);
 if(JS_DefinePropertyValueStr(c->ctx,v,"message",JS_DupValue(c->ctx,unpack(message)),JS_PROP_WRITABLE|JS_PROP_CONFIGURABLE|JS_PROP_THROW)<0) { JS_FreeValue(c->ctx,v); return pack(JS_EXCEPTION); }
 return pack(v);
}
/* The error's `name`: the first own `name` data property on the error or its prototype
 * chain. An accessor or non-string value ends the walk with undefined; a proxy ends it
 * before any trap runs. No application code executes. */
static JSValue error_name(jbq_context *c,JSValueConst v) {
 JSAtom a=JS_NewAtom(c->ctx,"name"); if(!a) return JS_EXCEPTION;
 JSValue o=JS_DupValue(c->ctx,v), out=JS_UNDEFINED;
 for(int depth=0;depth<64&&JS_IsObject(o)&&!JS_IsProxy(o);depth++) {
  JSPropertyDescriptor d; int r=JS_GetOwnProperty(c->ctx,&d,o,a);
  if(r<0) { out=JS_EXCEPTION; break; }
  if(r) {
   if(!(d.flags&JS_PROP_GETSET)&&JS_IsString(d.value)) out=JS_DupValue(c->ctx,d.value);
   JS_FreeValue(c->ctx,d.value); JS_FreeValue(c->ctx,d.getter); JS_FreeValue(c->ctx,d.setter);
   break;
  }
  JSValue next=JS_GetPrototype(c->ctx,o); JS_FreeValue(c->ctx,o); o=next;
  if(JS_IsException(o)) { out=JS_EXCEPTION; o=JS_UNDEFINED; break; }
 }
 JS_FreeValue(c->ctx,o); JS_FreeAtom(c->ctx,a); return out;
}
/**
 * Tells whether a thrown value is a host failure record ({ code, message, details }).
 *
 * Host failures reject as these records, not as Errors, and an Error-only read printed an
 * unhandled one as a bare "Uncaught exception". Decision: report a record's own `code` (as
 * the name) and `message` data properties; accessors are never called, so no application
 * code runs. Rejecting with real Errors from the engine bridge was tried and dropped: the
 * bridge is data-only and engine fixtures cannot construct objects.
 *
 * @param c Bridge context owning the value.
 * @param v Thrown value to classify.
 * @return 1 for an ordinary object (class id 1, JS_CLASS_OBJECT, so never a proxy or exotic
 *         object) whose prototype is null; 0 otherwise.
 */
static int failure_record(jbq_context *c,JSValueConst v) {
 if(!JS_IsObject(v)||JS_GetClassID(v)!=1) return 0;
 JSValue p=JS_GetPrototype(c->ctx,v); int r=JS_IsNull(p); JS_FreeValue(c->ctx,p); return r;
}
/**
 * Reads one own string data property of a failure record without running application code.
 *
 * @param c Bridge context owning the value.
 * @param v A value failure_record() accepted.
 * @param name Property name, "code" or "message".
 * @return The string (a new reference); undefined when the property is missing, an accessor
 *         or not a string; JS_EXCEPTION when the engine fails.
 */
static JSValue record_field(jbq_context *c,JSValueConst v,const char *name) {
 JSAtom a=JS_NewAtom(c->ctx,name); if(!a) return JS_EXCEPTION;
 JSPropertyDescriptor d; int r=JS_GetOwnProperty(c->ctx,&d,v,a); JS_FreeAtom(c->ctx,a);
 if(r<=0) return r<0?JS_EXCEPTION:JS_UNDEFINED;
 JSValue out=(!(d.flags&JS_PROP_GETSET)&&JS_IsString(d.value))?JS_DupValue(c->ctx,d.value):JS_UNDEFINED;
 JS_FreeValue(c->ctx,d.value); JS_FreeValue(c->ctx,d.getter); JS_FreeValue(c->ctx,d.setter); return out;
}
/**
 * Reads one diagnostic field of a thrown value for the host's failure report.
 *
 * @param c Bridge context owning the value.
 * @param value The thrown value.
 * @param stack Field selector: 0 message, 1 stack, 2 name. For a failure record the name is
 *        its code and there is no stack.
 * @return The field's string, or undefined when the value carries none; JS_EXCEPTION when
 *         the engine fails.
 */
jbq_value jbq_error_field(jbq_context *c,jbq_value value,int stack) {
 JSValue v=unpack(value);
 if(!JS_IsError(v)) {
  if(stack==1||!failure_record(c,v)) return pack(JS_UNDEFINED);
  return pack(record_field(c,v,stack==2?"code":"message"));
 }
 if(stack==2) return pack(error_name(c,v));
 JSAtom a=JS_NewAtom(c->ctx,stack?"stack":"message"); if(!a) return pack(JS_EXCEPTION);
 JSPropertyDescriptor d; int r=JS_GetOwnProperty(c->ctx,&d,v,a); JS_FreeAtom(c->ctx,a);
 if(r<0) return pack(JS_EXCEPTION);
 if(!r) {
  if(!stack) return pack(JS_UNDEFINED);
  /* Never look up or invoke the object's mutable stack accessor. An own
   * descriptor, including an accessor, suppresses this fallback entirely.
   * This saved original builtin reads only exact Error internal storage. */
  JSValue out=JS_Call(c->ctx,c->error_stack_getter,v,0,NULL);
  if(JS_IsString(out)||JS_IsException(out)) return pack(out);
  JS_FreeValue(c->ctx,out); return pack(JS_UNDEFINED);
 }
 JSValue out=JS_UNDEFINED;
 if(!(d.flags&JS_PROP_GETSET)&&JS_IsString(d.value)) out=JS_DupValue(c->ctx,d.value);
 JS_FreeValue(c->ctx,d.value); JS_FreeValue(c->ctx,d.getter); JS_FreeValue(c->ctx,d.setter); return pack(out);
}

jbq_value jbq_module_prepare(jbq_context *c,const uint16_t *s,size_t n,const char *origin) { return source(c,s,n,origin,2); }
int jbq_module_count(jbq_context *c,jbq_value v) { return JS_JetbunnyModuleCount(c->ctx,unpack(v)); }
jbq_value jbq_module_import(jbq_context *c,jbq_value v,int i) { return pack(JS_JetbunnyModuleImport(c->ctx,unpack(v),i)); }
int jbq_module_bind(jbq_context *c,jbq_value v,int i,jbq_value target) { return JS_JetbunnyModuleBind(c->ctx,unpack(v),i,unpack(target)); }
int jbq_module_link(jbq_context *c,jbq_value v) { return JS_JetbunnyModuleLink(c->ctx,unpack(v)); }
jbq_value jbq_module_namespace(jbq_context *c,jbq_value v) { return pack(JS_JetbunnyModuleNamespace(c->ctx,unpack(v))); }
int jbq_promise_state(jbq_context *c,jbq_value v) { return JS_PromiseState(c->ctx,unpack(v)); }
jbq_value jbq_promise_result(jbq_context *c,jbq_value v) { return pack(JS_PromiseResult(c->ctx,unpack(v))); }
uint8_t *jbq_module_write(jbq_context *c,jbq_value v,size_t *n) { return JS_WriteObject(c->ctx,n,unpack(v),JS_WRITE_OBJ_BYTECODE|JS_WRITE_OBJ_STRIP_SOURCE); }
uint8_t *jbq_module_cache_write(jbq_context *c,jbq_value v,size_t *n) { return JS_WriteObject(c->ctx,n,unpack(v),JS_WRITE_OBJ_BYTECODE); }
jbq_value jbq_module_read(jbq_context *c,const uint8_t *bytes,size_t n,jbq_value identity) {
 JSValue v=JS_ReadObject(c->ctx,bytes,n,JS_READ_OBJ_BYTECODE|JS_READ_OBJ_JETBUNNY_PREPARE);
 if(!JS_IsException(v) && JS_JetbunnyModuleRekey(c->ctx,v,unpack(identity))<0) { JS_FreeValue(c->ctx,v); return pack(JS_EXCEPTION); }
 return pack(v);
}
static int module_builtin_init(JSContext *ctx,JSModuleDef *m) {
 JSValue object=JS_GetModulePrivateValue(ctx,m); JSPropertyEnum *keys=NULL;uint32_t count=0;int status=-1;
 if(JS_GetOwnPropertyNames(ctx,&keys,&count,object,JS_GPN_STRING_MASK)<0) goto done;
 for(uint32_t i=0;i<count;i++) {
  const char *name=JS_AtomToCString(ctx,keys[i].atom);if(!name)goto done;
  JSValue value=JS_GetProperty(ctx,object,keys[i].atom);
  int result=JS_IsException(value)?-1:JS_SetModuleExport(ctx,m,name,value);
  JS_FreeCString(ctx,name);if(result<0)goto done;
 }
 status=0;
 done:JS_FreePropertyEnum(ctx,keys,count);JS_FreeValue(ctx,object);return status;
}
jbq_value jbq_module_builtin(jbq_context *c,const char *identity,jbq_value exports) {
 JSModuleDef *m=JS_NewCModule(c->ctx,identity,module_builtin_init);
 JSPropertyEnum *keys=NULL;uint32_t count=0;
 if(!m)return pack(JS_EXCEPTION);
 JS_SetModulePrivateValue(c->ctx,m,JS_DupValue(c->ctx,unpack(exports)));
 if(JS_GetOwnPropertyNames(c->ctx,&keys,&count,unpack(exports),JS_GPN_STRING_MASK)<0)goto fail;
 for(uint32_t i=0;i<count;i++) {
  const char *name=JS_AtomToCString(c->ctx,keys[i].atom);
  if(!name)goto fail;
  int status=JS_AddModuleExport(c->ctx,m,name);
  JS_FreeCString(c->ctx,name);
  if(status<0)goto fail;
 }
 JS_FreePropertyEnum(c->ctx,keys,count);
 return pack(JS_JetbunnyModuleValue(c->ctx,m));
 fail:
 JS_FreePropertyEnum(c->ctx,keys,count);
 JS_JetbunnyDiscardBuiltin(c->ctx,m);
 return pack(JS_EXCEPTION);
}

static void reject_import(JSContext *ctx,JSValue token,const char *message) {
 JSValue error=JS_NewString(ctx,message);
 if(JS_IsException(error))error=JS_GetException(ctx);
 JS_FinishDynamicImport(ctx,token,JS_EXCEPTION,error);
}
static void dynamic_import(JSContext *ctx,JSValueConst base,JSValueConst specifier,JSValue token,JSValueConst attributes,void *opaque) {
 jbq_context *c=opaque;
 /* Attributes (`with: { type: "json" }`) do not change resolution: the host picks a
  * module's kind from its file, as for static imports. */
 (void)attributes;
 size_t index=0;for(;index<128;index++)if(!c->imports[index].id)break;
 if(index==128 || c->next_import==UINT64_MAX || !JS_IsString(base) || !JS_IsString(specifier)) {
  reject_import(ctx,token,"dynamic import capacity or identity rejected");return;
 }
 c->imports[index]=(jbq_import){++c->next_import,token,JS_DupValue(ctx,base),JS_DupValue(ctx,specifier),0};
}
uint64_t jbq_import_take(jbq_context *c,jbq_value *base,jbq_value *specifier) {
 size_t selected=128;
 for(size_t i=0;i<128;i++) {
  if(c->imports[i].id && !c->imports[i].taken &&
     (selected==128 || c->imports[i].id<c->imports[selected].id))selected=i;
 }
 if(selected==128)return 0;
 jbq_import *request=&c->imports[selected];request->taken=1;
 *base=pack(JS_DupValue(c->ctx,request->base));
 *specifier=pack(JS_DupValue(c->ctx,request->specifier));
 return request->id;
}
static JSValue import_namespace(JSContext *ctx,JSValueConst receiver,int argc,JSValueConst *argv,int magic,JSValueConst *data) {
 (void)receiver;(void)argc;(void)argv;(void)magic;
 return JS_JetbunnyModuleNamespace(ctx,data[0]);
}
int jbq_import_finish(jbq_context *c,uint64_t id,jbq_value module,jbq_value evaluation,const char *failure) {
 size_t i=0;for(;i<128;i++)if(c->imports[i].id==id)break;
 if(i==128){JS_ThrowTypeError(c->ctx,"stale dynamic import");return -1;}
 jbq_import request=c->imports[i];memset(&c->imports[i],0,sizeof(c->imports[i]));
 JS_FreeValue(c->ctx,request.base);JS_FreeValue(c->ctx,request.specifier);
 if(failure){reject_import(c->ctx,request.token,failure);return 0;}
 JSValue data=unpack(module);
 JSValue callback=JS_NewCFunctionData(c->ctx,import_namespace,0,0,1,&data);
 if(JS_IsException(callback))return JS_FinishDynamicImport(c->ctx,request.token,JS_EXCEPTION,JS_GetException(c->ctx));
 JSValue promise=JS_PromiseThen(c->ctx,unpack(evaluation),callback,JS_UNDEFINED);JS_FreeValue(c->ctx,callback);
 if(JS_IsException(promise))return JS_FinishDynamicImport(c->ctx,request.token,JS_EXCEPTION,JS_GetException(c->ctx));
 return JS_FinishDynamicImport(c->ctx,request.token,promise,JS_UNDEFINED);
}

/* No source parsing or variable resolution lives in this adapter. */
static JSValue compiler_callback(JSContext *ctx,uint64_t lease,const uint16_t *source,size_t units,const char *filename,int line,int flags,void *opaque){
 jbq_context *c=opaque;
 if(!c->compiler||ctx!=c->ctx)return JS_ThrowTypeError(ctx,"compiler context mismatch");
 return unpack(c->compiler(c->compiler_opaque,c,lease,source,units,filename,line,
   flags|((c->debug_enabled&&!lease)?JS_EVAL_FLAG_DEBUG:0)));
}
int jbq_set_compiler(jbq_context *c,jbq_compiler compiler,void *opaque){
 if(JS_JetbunnySetCompiler(c->ctx,compiler?compiler_callback:NULL,compiler?c:NULL)<0)return -1;
 c->compiler=compiler;c->compiler_opaque=opaque;return 0;
}
int jbq_eval_environment(jbq_context *c,uint64_t lease,JSJetbunnyEnvView *out){return JS_JetbunnyEnvView(c->ctx,lease,out);}
int jbq_eval_binding(jbq_context *c,uint64_t lease,uint32_t kind,uint32_t index,JSJetbunnyEnvBinding *out){return JS_JetbunnyEnvBinding(c->ctx,lease,kind,index,out);}
jbq_value jbq_eval_binding_name(jbq_context *c,uint64_t lease,uint32_t kind,uint32_t index){return pack(JS_JetbunnyEnvName(c->ctx,lease,kind,index));}
jbq_value jbq_install_verified(jbq_context *c,const uint8_t *bytes,size_t len,int prepare){return pack(JS_JetbunnyInstall(c->ctx,bytes,len,prepare));}
jbq_value jbq_throw_compile(jbq_context *c,int kind,const char *message){
 if(kind==1)return pack(JS_ThrowSyntaxError(c->ctx,"%s",message));
 if(kind==2)return pack(JS_ThrowRangeError(c->ctx,"%s",message));
 if(kind==4)return pack(JS_ThrowTypeError(c->ctx,"%s",message));
 return pack(JS_ThrowInternalError(c->ctx,"%s",message));
}
int jbq_debug_eval_verified(jbq_context *c,int level,const uint16_t *source,size_t units,const char *filename,jbq_value *out){
 JSValue value=JS_UNDEFINED;
 int status=JS_JetbunnyDebugEvalControlled(c->ctx,level,source,units,filename,&value);
 *out=pack(value);return status;
}
int jbq_debug_lookup(jbq_context *c,int level,int root_this,const uint16_t *root,size_t units,
 const jbq_lookup_key *keys,size_t count,jbq_value *out){
 _Static_assert(sizeof(jbq_lookup_key)==sizeof(JSJetbunnyLookupKey),"lookup key ABI");
 JSValue value=JS_UNDEFINED;
 int status=JS_JetbunnyDebugLookup(c->ctx,level,root_this,root,units,keys,count,&value);
 *out=pack(value);return status;
}
int jbq_debug_unwinding(jbq_context *c){return JS_JetbunnyDebugUnwinding(c->ctx);}

const void *jbq_inspect_root(jbq_context *c,jbq_value value){return JS_JetbunnyInspectRoot(c->ctx,unpack(value));}
int jbq_inspect_view(jbq_context *c,const void *f,JSJetbunnyGraphView *v){return JS_JetbunnyInspectView(c->ctx,f,v);}
int jbq_inspect_constant(jbq_context *c,const void *f,uint32_t i,JSJetbunnyValueView *v){return JS_JetbunnyInspectConstant(c->ctx,f,i,v);}
int jbq_inspect_local(jbq_context *c,const void *f,uint32_t i,JSJetbunnyLocalView *v){return JS_JetbunnyInspectLocal(c->ctx,f,i,v);}
int jbq_inspect_capture(jbq_context *c,const void *f,uint32_t i,JSJetbunnyCaptureView *v){return JS_JetbunnyInspectCapture(c->ctx,f,i,v);}
int jbq_inspect_text(jbq_context *c,const void *f,uint32_t kind,uint32_t i,uint16_t *out,size_t cap){return JS_JetbunnyInspectText(c->ctx,f,kind,i,out,cap);}
int jbq_inspect_bigint(jbq_context *c,const void *f,uint32_t i,uint8_t *out,size_t cap){return JS_JetbunnyInspectBigInt(c->ctx,f,i,out,cap);}
int jbq_inspect_module(jbq_context *c,jbq_value value,JSJetbunnyModuleView *out){return JS_JetbunnyInspectModule(c->ctx,unpack(value),out);}
int jbq_inspect_module_entry(jbq_context *c,jbq_value value,uint32_t kind,uint32_t index,JSJetbunnyModuleEntry *out){return JS_JetbunnyInspectModuleEntry(c->ctx,unpack(value),kind,index,out);}

/* Owner-thread native debugger adapter. All names/results owned except values
 * explicitly borrowed by the enumeration callback. */
static int debug_hook(JSContext *ctx,JSDebugEvent event,void *opaque) {
 (void)ctx; jbq_context *c=opaque;
 int result=c->debug_hook?c->debug_hook(c->debug_opaque,c,(int)event):0;
 if(result>0) { c->debug_stopped=1; JS_DebugStop(c->ctx); return -1; }
 return result;
}
void jbq_debug_install(jbq_context *c,jbq_debug_hook hook,void *opaque) {
 c->debug_hook=hook;c->debug_opaque=opaque;c->debug_enabled=hook!=NULL;
 JS_SetDebugHook(c->ctx,hook?debug_hook:NULL,hook?c:NULL);
 if(!hook)JS_SetDebugFlags(c->ctx,0);
}
void jbq_debug_flags(jbq_context *c,int flags){JS_SetDebugFlags(c->ctx,flags);}
typedef struct {jbq_debug_marker callback;void *opaque;} debug_marker_state;
static void debug_marker(void *opaque,void *fn,uint32_t marker,int line,int col,bool debugger) {
 debug_marker_state *s=opaque;s->callback(s->opaque,(uintptr_t)fn,marker,line,col,debugger);
}
int jbq_debug_markers(jbq_context *c,jbq_value root,jbq_debug_marker callback,void *opaque) {
 if(!callback)return -1;
 debug_marker_state s={callback,opaque};return JS_DebugForEachMarker(c->ctx,unpack(root),debug_marker,&s);
}
int jbq_debug_breakpoint(jbq_context *c,uintptr_t function,uint32_t marker,int enabled) {
 return JS_DebugSetBreakpoint(c->ctx,(void *)function,marker,enabled!=0);
}
int jbq_debug_frame_at(jbq_context *c,int level,jbq_debug_frame *out,jbq_value *name,jbq_value *origin) {
 JSDebugFrame frame;if(JS_DebugGetFrame(c->ctx,level,&frame)<0)return -1;
 memset(out,0,sizeof(*out));out->function=(uintptr_t)frame.fn;out->activation=frame.activation;
 out->marker=frame.marker;out->line=frame.line;out->column=frame.col;out->scope_known=frame.scope_known;
 *name=pack(frame.func_name?JS_AtomToString(c->ctx,frame.func_name):JS_UNDEFINED);
 *origin=pack(frame.filename?JS_AtomToString(c->ctx,frame.filename):JS_UNDEFINED);
 return 0;
}
typedef struct {jbq_context *context;jbq_debug_variable callback;void *opaque;} debug_variable_state;
static void debug_variable(void *opaque,JSAtom name,JSValueConst value,JSDebugVarKind kind,int depth) {
 debug_variable_state *s=opaque;JSValue text=JS_AtomToString(s->context->ctx,name);
 s->callback(s->opaque,pack(text),pack(value),(int)kind,depth);JS_FreeValue(s->context->ctx,text);
}
int jbq_debug_variables(jbq_context *c,int level,jbq_debug_variable callback,void *opaque) {
 if(!callback)return -1;debug_variable_state s={c,callback,opaque};
 return JS_DebugGetFrameVars(c->ctx,level,debug_variable,&s);
}
jbq_value jbq_debug_stop(jbq_context *c){c->debug_stopped=1;return pack(JS_DebugStop(c->ctx));}
int jbq_take_debug_stop(jbq_context *c){int stopped=c->debug_stopped;c->debug_stopped=0;return stopped;}

void jbq_debug_exception_policy(jbq_context *c,int policy){JS_DebugSetExceptionPolicy(c->ctx,policy);}
int jbq_debug_exception(jbq_context *c,jbq_value *out,int *caught){
 JSValue value;bool handled;
 if(!out||!caught||JS_DebugGetException(c->ctx,&value,&handled)<0)return -1;
 *out=pack(value);*caught=handled;return 0;
}
int jbq_debug_frame_count(jbq_context *c){
 JSDebugFrame frame;int count=0;
 while(count<65536&&JS_DebugGetFrame(c->ctx,count,&frame)==0)count++;
 return count==65536?-1:count;
}
int jbq_debug_uninitialized(jbq_value value){return JS_VALUE_GET_TAG(unpack(value))==JS_TAG_UNINITIALIZED;}

/* Bounded, non-executing debugger own-property pages. Never route inspection
 * through JS_GetProperty: accessors are metadata, not evaluated values. */
int jbq_debug_object_class(jbq_context *c,jbq_value value) {
 return JS_DebugObjectClass(c->ctx,unpack(value));
}

int jbq_debug_value_info_get(jbq_context *c,jbq_value value,jbq_debug_value_info *out) {
 JSDebugValueInfo info;
 _Static_assert(sizeof(info)==sizeof(*out), "debug value info ABI");
 int result=JS_DebugGetValueInfo(c->ctx,unpack(value),&info);
 memcpy(out,&info,sizeof(info));return result;
}
int jbq_debug_exception_description(jbq_context *c,jbq_debug_value_info *out) {
 JSDebugValueInfo info;
 int result=JS_DebugGetExceptionDescription(c->ctx,&info);
 if(result==1)memcpy(out,&info,sizeof(info));return result;
}
int jbq_debug_copy_string(jbq_value value,uint16_t *out,size_t cap,size_t *length) {
 return JS_DebugCopyString(unpack(value),out,cap,length);
}
typedef struct { jbq_context *context; jbq_debug_page_entry callback; void *opaque; } debug_page_state;
static int debug_page_entry(void *opaque,JSAtom atom,JSValueConst value,int flags,uint32_t kind,uint32_t index) {
 debug_page_state *s=opaque;
 JSValue name=JS_AtomToString(s->context->ctx,atom);
 if(JS_IsException(name))return -1;
 int result=s->callback(s->opaque,s->context,pack(name),pack(value),flags,atom,kind,index);
 JS_FreeValue(s->context->ctx,name);return result;
}
void jbq_debug_cursor_retain(jbq_debug_cursor *cursor) {
 JSDebugCursor native;
 _Static_assert(sizeof(native)==sizeof(*cursor), "debug cursor ABI");
 memcpy(&native,cursor,sizeof(native));
 JS_DebugRetainCursor(&native);
 memcpy(cursor,&native,sizeof(native));
}
void jbq_debug_cursor_release(jbq_context *c,jbq_debug_cursor *cursor) {
 JSDebugCursor native;
 _Static_assert(sizeof(native)==sizeof(*cursor), "debug cursor ABI");
 memcpy(&native,cursor,sizeof(native));
 JS_DebugReleaseCursor(c->ctx,&native);
 memcpy(cursor,&native,sizeof(native));
}
int jbq_debug_inspect(jbq_context *c,jbq_value object,int frame,uint32_t scope,
                      jbq_debug_cursor *cursor,size_t count,size_t scan,
                      jbq_debug_page_entry callback,void *opaque,size_t *scanned,int *complete) {
 JSDebugCursor native;
 _Static_assert(sizeof(native)==sizeof(*cursor), "debug cursor ABI");
 memcpy(&native,cursor,sizeof(native));
 debug_page_state s={c,callback,opaque};
 int result=frame<0?JS_DebugPropertyPage(c->ctx,unpack(object),&native,count,scan,debug_page_entry,&s,scanned,complete):
 JS_DebugScopePage(c->ctx,frame,scope,&native,count,scan,debug_page_entry,&s,scanned,complete);
 memcpy(cursor,&native,sizeof(native));return result;
}

int jbq_debug_properties(jbq_context *c,jbq_value value,size_t start,size_t count,
                         jbq_debug_property callback,void *opaque,size_t *total) {
 JSValue object=unpack(value);
 size_t bound=0; *total=0;
 if(!callback||JS_DebugOwnPropertyBound(c->ctx,object,&bound)<0) return -1;
 if(bound>65536||count>1024) return -2;
 JSPropertyEnum *keys=NULL;uint32_t length=0;
 if(JS_GetOwnPropertyNames(c->ctx,&keys,&length,object,
       JS_GPN_STRING_MASK|JS_GPN_SYMBOL_MASK)<0) return -1;
 *total=length;
 size_t end=start>length?start:start+(count<(size_t)length-start?count:(size_t)length-start);
 int status=0;
 for(size_t i=start;i<end&&i<length;i++) {
  JSPropertyDescriptor descriptor;
  int found=JS_GetOwnProperty(c->ctx,&descriptor,object,keys[i].atom);
  if(found<0) {status=-1;break;}
  if(!found) {status=-1;break;}
  JSValue name=JS_AtomToValue(c->ctx,keys[i].atom);
  int symbol=JS_IsSymbol(name);
  if(symbol) {
   /* Raw atom description, never Symbol.prototype getters or coercion. */
   JS_FreeValue(c->ctx,name);
   name=JS_AtomToString(c->ctx,keys[i].atom);
  }
  if(JS_IsException(name)) status=-1;
  else {
   int accessor=(descriptor.flags&JS_PROP_GETSET)!=0;
   callback(opaque,c,pack(name),pack(accessor?JS_UNDEFINED:descriptor.value),accessor|(symbol?2:0));
   JS_FreeValue(c->ctx,name);
  }
  JS_FreeValue(c->ctx,descriptor.value);
  JS_FreeValue(c->ctx,descriptor.getter);
  JS_FreeValue(c->ctx,descriptor.setter);
  if(status<0) break;
 }
 JS_FreePropertyEnum(c->ctx,keys,length);
 return status;
}

jbq_value jbq_compile_debug(jbq_context *c,const uint16_t *s,size_t n,const char *origin) { return source(c,s,n,origin,3); }
uint8_t *jbq_write_debug(jbq_context *c,jbq_value v,size_t *n) { return JS_WriteObject(c->ctx,n,unpack(v),JS_WRITE_OBJ_BYTECODE|JS_WRITE_OBJ_STRIP_SOURCE); }

void *jbq_debug_target_new(jbq_context *c,jbq_value object,int frame,uint32_t scope,uint32_t atom,uint32_t kind,uint32_t index,int *status) {
 return JS_JetbunnyTargetNew(c->ctx,unpack(object),frame,scope,atom,kind,index,status);
}
void jbq_debug_target_free(jbq_context *c,void *target) { JS_JetbunnyTargetFree(c->ctx,target); }
int jbq_debug_target_validate(jbq_context *c,void *target) { return JS_JetbunnyTargetValidate(c->ctx,target); }
int jbq_debug_target_store(jbq_context *c,void *target,jbq_value value) { return JS_JetbunnyTargetStore(c->ctx,target,unpack(value)); }

int jbq_debug_step_info(jbq_context *c,uint64_t expected,uint64_t *activation,int *kind,uint64_t *parent) {
 return JS_DebugStepInfo(c->ctx,expected,activation,kind,parent);
}

static uint64_t debug_async_hook(JSContext *ctx,int kind,uint64_t parent,void *opaque) {
 (void)ctx;jbq_context *c=opaque;
 return c->debug_async_hook?c->debug_async_hook(c->debug_async_opaque,c,kind,parent):0;
}
void jbq_debug_async_install(jbq_context *c,jbq_debug_async_hook hook,void *opaque) {
 c->debug_async_hook=hook;c->debug_async_opaque=opaque;
 JS_SetDebugAsyncHook(c->ctx,hook?debug_async_hook:NULL,hook?c:NULL);
}
uint64_t jbq_debug_async_capture(jbq_context *c,int kind){return JS_DebugCaptureAsync(c->ctx,kind);}
uint64_t jbq_debug_async_stack(jbq_context *c){return JS_DebugAsyncStack(c->ctx);}
uint64_t jbq_debug_async_set(jbq_context *c,uint64_t id){return JS_DebugSetAsyncStack(c->ctx,id);}
int jbq_debug_frame_metadata(jbq_context *c,int level,jbq_debug_frame *out) {
 JSDebugFrame frame;if(JS_DebugGetFrame(c->ctx,level,&frame)<0)return -1;
 memset(out,0,sizeof(*out));out->function=(uintptr_t)frame.fn;out->activation=frame.activation;
 out->marker=frame.marker;out->line=frame.line;out->column=frame.col;out->scope_known=frame.scope_known;
 return 0;
}

void jbq_debug_rejection_install(jbq_context *c,jbq_debug_rejection_hook hook,void *opaque) {
 c->debug_rejection_hook=hook;c->debug_rejection_opaque=opaque;
}
/* lunarstorm patch: the debugger half of track_rejection, for a wrapped context
 * whose owner installed its own rejection tracker (jbq_context_wrap); the owner
 * calls it from that tracker with the engine's arguments. */
void jbq_debug_rejection(jbq_context *c,void *promise,void *reason,int handled) {
 if(!c||!c->debug_rejection_hook) return;
 JSValue p,r; memcpy(&p,promise,sizeof(p)); memcpy(&r,reason,sizeof(r));
 uint64_t history=handled?0:JS_DebugPromiseRejectionOrigin(p);
 c->debug_rejection_hook(c->debug_rejection_opaque,c,pack(p),pack(r),handled,history);
}
uintptr_t jbq_debug_promise_identity(jbq_context *c,jbq_value value) {
 JSValue v=unpack(value);
 return JS_IsObject(v)&&JS_PromiseState(c->ctx,v)>=0?(uintptr_t)JS_VALUE_GET_PTR(v):0;
}

int jbq_debug_describe_exception(jbq_context *c,jbq_value value,jbq_debug_value_info *out) {
 JSDebugValueInfo info;int result=JS_DebugDescribeException(c->ctx,unpack(value),&info);
 if(result==1)memcpy(out,&info,sizeof(info));return result;
}
