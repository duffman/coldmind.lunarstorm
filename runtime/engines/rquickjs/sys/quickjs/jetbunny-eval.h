/* Private pinned-engine ABI. Owner-thread only; no callback may unwind.
 * Compiler callbacks return an OWNED compiled object, never execute it, and
 * must install only Rust-verified images after independent native inspection.
 * Lease IDs are synchronous capabilities, never compiler inputs/artifacts. */
#ifndef JETBUNNY_EVAL_H
#define JETBUNNY_EVAL_H
#include "quickjs.h"
typedef struct {
 uint32_t args,locals,captures,var_refs,strict_mode,flags;
 int32_t scope_head;
} JSJetbunnyEnvView;
typedef struct {
 uint32_t name,scope_level;int32_t scope_next;
 uint32_t flags,var_ref_idx,initialized,live;
} JSJetbunnyEnvBinding;
/* view flags bit 11: script-root frame (sloppy eval vars are global). */
/* binding kind: 0=argument, 1=local, 2=existing closure reference.
 * local flags: const|lexical<<1|captured<<2|var_kind<<3.
 * ref flags: closure_type|const<<3|lexical<<4|var_kind<<5. */
typedef JSValue JSJetbunnyCompiler(JSContext *,uint64_t lease,
 const uint16_t *source,size_t units,const char *filename,int line,int flags,void *);
typedef JSValue JSJetbunnyFrameCallback(JSContext *,uint64_t lease,void *);
JS_EXTERN int JS_JetbunnySetCompiler(JSContext *,JSJetbunnyCompiler *,void *);
JS_EXTERN JSValue JS_JetbunnyWithFrame(JSContext *,int,JSJetbunnyFrameCallback *,void *);
JS_EXTERN int JS_JetbunnyEnvView(JSContext *,uint64_t,JSJetbunnyEnvView *);
JS_EXTERN int JS_JetbunnyEnvBinding(JSContext *,uint64_t,uint32_t,uint32_t,JSJetbunnyEnvBinding *);
JS_EXTERN JSValue JS_JetbunnyEnvName(JSContext *,uint64_t,uint32_t,uint32_t);
/* Unsafe native boundary: caller proves immutable image target/schema/graph.
 * `prepare` must be nonzero for module graph installation. */
JS_EXTERN JSValue JS_JetbunnyInstall(JSContext *,const uint8_t *,size_t,int prepare);
/* Consumes compiled even on rejection. Checks live lease and capture bounds. */
JS_EXTERN JSValue JS_JetbunnyEvalFrame(JSContext *,uint64_t,JSValue compiled);
JS_EXTERN JSValue JS_JetbunnyDebugEvalUTF16(JSContext *,int,const uint16_t *,size_t,const char *);
/* Controlled status: 0 returned, 1 threw, 2 cancelled, 3 expired, 4 stop,
 * 5 allocation failure. out owns the evaluator value, captured BEFORE the
 * paused application's exception, trace and OOM metadata are restored. */
JS_EXTERN int JS_JetbunnyDebugEvalControlled(JSContext *,int,const uint16_t *,size_t,const char *,JSValue *out);
JS_EXTERN bool JS_JetbunnyDebugUnwinding(JSContext *);
JS_EXTERN int JS_JetbunnyDebugPoll(JSContext *);
/* Targets own an exact atom and receiver/function root. No slot pointer escapes.
 * kind: 1 own data property, 2 argument, 3 local, 4 closure. Store borrows RHS. */
JS_EXTERN void *JS_JetbunnyTargetNew(JSContext *,JSValueConst,int,uint32_t,JSAtom,uint32_t,uint32_t,int *);
JS_EXTERN void JS_JetbunnyTargetFree(JSContext *,void *);
JS_EXTERN int JS_JetbunnyTargetValidate(JSContext *,void *);
JS_EXTERN int JS_JetbunnyTargetStore(JSContext *,void *,JSValueConst);
/* Non-executing lookup. Borrowed literal keys; root_this bypasses only the
 * hidden-this filter. 0=owned value, 1=absent, 2=unavailable, -1=frame proof,
 * -2=capacity. No syntax/evaluation enters this interface. */
typedef struct { const uint16_t *text; size_t units; uint32_t index, is_index; } JSJetbunnyLookupKey;
JS_EXTERN int JS_JetbunnyDebugLookup(JSContext *,int,int,const uint16_t *,size_t,
 const JSJetbunnyLookupKey *,size_t,JSValue *);
#endif
