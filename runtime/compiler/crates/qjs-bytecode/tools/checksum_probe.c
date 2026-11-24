/* Test oracle: use the exact pinned native routine, never the source compiler. */
#include "quickjs.c"
/* Test-only compatibility consumer: no frame handle leaves this callback. */
static const uint8_t *eval_bytes;
static size_t eval_length;
static JSValue probe_eval(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val; (void)argc; (void)argv;
    JSStackFrame *sf = ctx->rt->current_stack_frame;
    while (sf && (JS_VALUE_GET_TAG(sf->cur_func) != JS_TAG_OBJECT ||
           !js_class_has_bytecode(JS_VALUE_GET_OBJ(sf->cur_func)->class_id))) sf = sf->prev_frame;
    if (!sf) return JS_ThrowInternalError(ctx, "missing test frame");
    JSObject *p = JS_VALUE_GET_OBJ(sf->cur_func);
    JSValue image = JS_ReadObject(p->u.func.function_bytecode->realm, eval_bytes, eval_length, JS_READ_OBJ_BYTECODE);
    if (JS_IsException(image)) return image;
    return JS_EvalFunctionInternal(p->u.func.function_bytecode->realm, image, JS_UNDEFINED, p->u.func.var_refs, sf);
}
static int marker_hits;
static void probe_marker_var(void *opaque, JSAtom name, JSValueConst value, JSDebugVarKind kind, int depth) {
    (void)name;
    int *count = opaque;
    if (kind == JS_DEBUG_VAR_LEXICAL && depth == 0 && JS_VALUE_GET_TAG(value) == JS_TAG_INT && JS_VALUE_GET_INT(value) == 42) ++*count;
}
static int probe_marker(JSContext *ctx, JSDebugEvent event, void *opaque) {
    (void)opaque;
    JSDebugFrame frame; int values = 0;
    if (event != JS_DEBUG_EVENT_STATEMENT || !ctx->debug_in_hook || !ctx->debug_pause_sf || ctx->debug_pause_scope != 0) return -1;
    if (JS_DebugGetFrame(ctx, 0, &frame) || frame.line != 4 || frame.col != 8) return -1;
    if (JS_DebugGetFrameVars(ctx, 0, probe_marker_var, &values) || values != 1) return -1;
    ++marker_hits; return 0;
}
int main(int argc, char **argv) {
    (void)argv;
    uint8_t input[4096];
    size_t n=fread(input,1,sizeof(input),stdin);
    if(ferror(stdin)||!feof(stdin))return 2;
    if (argc > 1) {
        JSRuntime *rt = JS_NewRuntime();
        JSContext *ctx = JS_NewContext(rt);
        bool graph_mode = strcmp(argv[1], "graph") == 0;
        bool eval_mode = strcmp(argv[1], "eval") == 0;
        bool marker_mode = strcmp(argv[1], "marker") == 0;
        if (marker_mode) {JS_SetDebugHook(ctx, probe_marker, NULL); JS_SetDebugFlags(ctx, JS_DEBUG_STEP);}
        JSValue dependency = JS_UNDEFINED;
        size_t offset = 0;
        if (graph_mode) {
            if (n < 4) return 6;
            uint32_t length = get_u32(input);
            if (length > n - 4) return 6;
            dependency = JS_ReadObject(ctx, input + 4, length, JS_READ_OBJ_BYTECODE);
            if (JS_IsException(dependency)) return 7;
            offset = 4 + length;
        }
        size_t root_length = n - offset;
        if (eval_mode) {
            if (n < 4 || get_u32(input) > n - 4) return 8;
            root_length = get_u32(input); offset = 4;
            eval_bytes = input + offset + root_length;
            eval_length = n - offset - root_length;
            JSValue global = JS_GetGlobalObject(ctx);
            JS_SetPropertyStr(ctx, global, "probeEval", JS_NewCFunction(ctx, probe_eval, "probeEval", 0));
            JS_FreeValue(ctx, global);
        }
        JSValue value = JS_ReadObject(ctx, input + offset, root_length, JS_READ_OBJ_BYTECODE);
        bool module_mode = graph_mode || strcmp(argv[1], "module") == 0;
        JSValue module_value = module_mode ? JS_DupValue(ctx, value) : JS_UNDEFINED;
        if (!JS_IsException(value)) value = JS_EvalFunction(ctx, value);
        if (JS_IsException(value)) {
            JSValue exception = JS_GetException(ctx);
            const char *message = JS_ToCString(ctx, exception);
            fprintf(stderr, "%s\n", message ? message : "native exception");
            JS_FreeCString(ctx, message);
            JS_FreeValue(ctx, exception);
            JS_FreeContext(ctx); JS_FreeRuntime(rt); return 3;
        }
        JSValue forwarded = JS_UNDEFINED;
        if (module_mode) {
            if (JS_PromiseState(ctx, value) == JS_PROMISE_REJECTED) return 5;
            JS_FreeValue(ctx, value);
            JSValue ns = JS_GetModuleNamespace(ctx, JS_VALUE_GET_PTR(module_value));
            value = JS_GetPropertyStr(ctx, ns, "answer");
            if (graph_mode) forwarded = JS_GetPropertyStr(ctx, ns, "forwarded");
            JS_FreeValue(ctx, ns); JS_FreeValue(ctx, module_value);
        }
        const char *result = JS_ToCString(ctx, value);
        if (!result) return 4;
        if (graph_mode) {
            const char *other = JS_ToCString(ctx, forwarded);
            printf("%s %s\n", result, other ? other : "exception");
            JS_FreeCString(ctx, other); JS_FreeValue(ctx, forwarded);
        } else if (marker_mode) printf("%s %d\n", result, marker_hits);
        else printf("%s\n", result);
        JS_FreeValue(ctx, dependency);
        JS_FreeCString(ctx, result); JS_FreeValue(ctx, value);
        JS_FreeContext(ctx); JS_FreeRuntime(rt);
    } else printf("%u\n",bc_csum(input,n));
    return 0;
}
