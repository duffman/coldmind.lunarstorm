/* Source-module integration oracle: pinned native VM, with its parser disabled.
 * Prepare every record before resolving edges so cycles do not trigger host IO. */
#include "quickjs.c"

static void show_exception(JSContext *ctx, JSValue value) {
    const char *message = JS_ToCString(ctx, value);
    fprintf(stderr, "%s\n", message ? message : "native exception");
    JS_FreeCString(ctx, message);
    JS_FreeValue(ctx, value);
}
int main(int argc, char **argv) {
    uint8_t bytes[4096];
    size_t size = fread(bytes, 1, sizeof(bytes), stdin);
    if (ferror(stdin) || !feof(stdin)) return 2;
    bool graph = argc > 1 && strcmp(argv[1], "graph") == 0;
    bool script = argc > 1 && strcmp(argv[1], "script") == 0;
    JSRuntime *rt = JS_NewRuntime();
    JSContext *ctx = JS_NewContext(rt);
    JSValue dependency = JS_UNDEFINED, module = JS_UNDEFINED;
    JSValue value = JS_UNDEFINED, ns = JS_UNDEFINED;
    size_t offset = 0;
    int status = 0;
    if (graph) {
        if (size < 4 || get_u32(bytes) > size - 4) { status = 2; goto done; }
        offset = 4 + get_u32(bytes);
        dependency = JS_ReadObject(ctx, bytes + 4, offset - 4,
                                  JS_READ_OBJ_BYTECODE | JS_READ_OBJ_JETBUNNY_PREPARE);
        if (JS_IsException(dependency)) { status = 3; goto exception; }
    }
    module = JS_ReadObject(ctx, bytes + offset, size - offset,
                           JS_READ_OBJ_BYTECODE | JS_READ_OBJ_JETBUNNY_PREPARE);
    if (JS_IsException(module) || JS_ResolveModule(ctx, module) < 0) {
        status = 3; goto exception;
    }
    value = JS_EvalFunction(ctx, JS_DupValue(ctx, module));
    if (JS_IsException(value)) { status = 3; goto exception; }
    if (JS_PromiseState(ctx, value) == JS_PROMISE_REJECTED) {
        show_exception(ctx, JS_PromiseResult(ctx, value)); status = 5; goto done;
    }
    if (!script) {
        ns = JS_GetModuleNamespace(ctx, JS_VALUE_GET_PTR(module));
        if (JS_IsException(ns)) { status = 3; goto exception; }
    }
    JSValue answer = script ? JS_DupValue(ctx, value) : JS_GetPropertyStr(ctx, ns, "answer");
    const char *text = JS_ToCString(ctx, answer);
    if (!text) { JS_FreeValue(ctx, answer); status = 3; goto exception; }
    printf("%s", text);
    JS_FreeCString(ctx, text); JS_FreeValue(ctx, answer);
    if (graph) {
        JSValue forwarded = JS_GetPropertyStr(ctx, ns, "forwarded");
        text = JS_ToCString(ctx, forwarded);
        if (!text) { JS_FreeValue(ctx, forwarded); status = 3; goto exception; }
        printf(" %s", text);
        JS_FreeCString(ctx, text); JS_FreeValue(ctx, forwarded);
    }
    printf("\n");
    goto done;
exception:
    show_exception(ctx, JS_GetException(ctx));
done:
    JS_FreeValue(ctx, ns); JS_FreeValue(ctx, value);
    JS_FreeValue(ctx, module); JS_FreeValue(ctx, dependency);
    JS_FreeContext(ctx); JS_FreeRuntime(rt);
    return status;
}
