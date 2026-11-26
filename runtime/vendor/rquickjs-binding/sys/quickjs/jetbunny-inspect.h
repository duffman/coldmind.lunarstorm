/* Private, read-only pinned-engine metadata ABI. Borrowed opaque handles only;
 * caller keeps the owner-context compiled value rooted for the entire copy. */
#ifndef JETBUNNY_INSPECT_H
#define JETBUNNY_INSPECT_H
#include "quickjs.h"
typedef struct {
 const uint8_t *code,*positions;size_t code_len,positions_len;
 uint32_t locals,args,defined_args,stack,constants,strict_mode,flags,captures,var_refs;
 uint32_t name,filename,line,column;
} JSJetbunnyGraphView;
typedef struct {uint32_t name,scope_level;int32_t scope_next;uint32_t flags,var_ref_idx;} JSJetbunnyLocalView;
typedef struct {uint32_t name,var_idx,flags;} JSJetbunnyCaptureView;
/* kind0 number,1 bool,2 undefined,3 null,4 UTF16 string,5 nested function,6 BigInt. */
typedef struct {uint32_t kind;double number;const void *function;} JSJetbunnyValueView;
typedef struct {uint32_t name,required,exports,stars,imports,has_tla;} JSJetbunnyModuleView;
typedef struct {uint32_t kind,module,capture,name,import_name;} JSJetbunnyModuleEntry;
JS_EXTERN const void *JS_JetbunnyInspectRoot(JSContext *,JSValueConst);
JS_EXTERN int JS_JetbunnyInspectView(JSContext *,const void *,JSJetbunnyGraphView *);
JS_EXTERN int JS_JetbunnyInspectValue(JSContext *,JSValueConst,JSJetbunnyValueView *);
JS_EXTERN int JS_JetbunnyInspectConstant(JSContext *,const void *,uint32_t,JSJetbunnyValueView *);
/* Local accessor returns1 only when the native function has no vardef table. */
JS_EXTERN int JS_JetbunnyInspectLocal(JSContext *,const void *,uint32_t,JSJetbunnyLocalView *);
JS_EXTERN int JS_JetbunnyInspectCapture(JSContext *,const void *,uint32_t,JSJetbunnyCaptureView *);
JS_EXTERN int JS_JetbunnyInspectText(JSContext *,const void *,uint32_t,uint32_t,uint16_t *,size_t);
JS_EXTERN int JS_JetbunnyInspectString(JSContext *,JSValueConst,uint16_t *,size_t);
JS_EXTERN int JS_JetbunnyInspectBigInt(JSContext *,const void *,uint32_t,uint8_t *,size_t);
/* Module view returns1 for a function,0 for a module,-1 on invalid metadata.
 * Entry kinds:0 required,1 export (kind0 local/1 indirect),2 star,3 import. */
JS_EXTERN int JS_JetbunnyInspectModule(JSContext *,JSValueConst,JSJetbunnyModuleView *);
JS_EXTERN int JS_JetbunnyInspectModuleEntry(JSContext *,JSValueConst,uint32_t,uint32_t,JSJetbunnyModuleEntry *);
#endif
