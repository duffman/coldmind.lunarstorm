/* Jetbunny private module extension ABI, pinned to the vendored engine revision.
 * Call only on the owner thread with a live context. Returned JSValues are owned.
 * Images must come from an authorized trusted compiler, never arbitrary bytes. */
#ifndef JETBUNNY_MODULES_H
#define JETBUNNY_MODULES_H
#include "quickjs.h"
#ifdef __cplusplus
extern "C" {
#endif
/* Lossless counterpart to upstream's C-string dynamic import callback. */
typedef void JSJetbunnyDynamicImportFunc(JSContext *, JSValueConst base,
                                       JSValueConst specifier, JSValue token,
                                       JSValueConst attributes, void *opaque);
JS_EXTERN void JS_JetbunnySetDynamicImportFunc(JSRuntime *, JSJetbunnyDynamicImportFunc *, void *);
#define JS_EVAL_FLAG_JETBUNNY_PREPARE (1 << 9)
#define JS_READ_OBJ_JETBUNNY_PREPARE (1 << 8)
JS_EXTERN int JS_JetbunnyModuleCount(JSContext *, JSValueConst);
JS_EXTERN JSValue JS_JetbunnyModuleImport(JSContext *, JSValueConst, int);
JS_EXTERN int JS_JetbunnyModuleBind(JSContext *, JSValueConst, int, JSValueConst);
JS_EXTERN int JS_JetbunnyModuleLink(JSContext *, JSValueConst);
JS_EXTERN int JS_JetbunnyModuleRekey(JSContext *, JSValueConst, JSValueConst);
JS_EXTERN JSValue JS_JetbunnyModuleValue(JSContext *, JSModuleDef *);
JS_EXTERN JSValue JS_JetbunnyModuleNamespace(JSContext *, JSValueConst);
JS_EXTERN void JS_JetbunnyDiscardBuiltin(JSContext *, JSModuleDef *);
#ifdef __cplusplus
}
#endif
#endif
