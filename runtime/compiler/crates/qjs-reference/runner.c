/* SPDX-License-Identifier: MIT */
#include "quickjs.h"

#include <errno.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define EXECUTION_BUDGET_MS 1000

struct execution_budget {
    struct timespec start;
    int exceeded;
};

static int interrupt_execution(JSContext *ctx, void *opaque)
{
    (void)ctx;
    struct execution_budget *budget = opaque;
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        budget->exceeded = 1;
        return 1;
    }
    int64_t elapsed_ns = (int64_t)(now.tv_sec - budget->start.tv_sec) * 1000000000
        + now.tv_nsec - budget->start.tv_nsec;
    if (elapsed_ns >= (int64_t)EXECUTION_BUDGET_MS * 1000000) {
        budget->exceeded = 1;
        return 1;
    }
    return 0;
}

/* JS_Eval requires a trailing NUL even though it receives an explicit length. */
static char *read_source(FILE *input, size_t *length)
{
    size_t capacity = 4096;
    size_t used = 0;
    char *source = malloc(capacity);
    if (!source)
        return NULL;
    for (;;) {
        if (used == capacity - 1) {
            if (capacity > SIZE_MAX / 2) {
                free(source);
                errno = EOVERFLOW;
                return NULL;
            }
            capacity *= 2;
            char *grown = realloc(source, capacity);
            if (!grown) {
                free(source);
                return NULL;
            }
            source = grown;
        }
        size_t count = fread(source + used, 1, capacity - used - 1, input);
        used += count;
        if (count == 0) {
            if (ferror(input)) {
                free(source);
                return NULL;
            }
            break;
        }
    }
    source[used] = '\0';
    *length = used;
    return source;
}

static void report_exception(JSContext *ctx)
{
    JSValue exception = JS_GetException(ctx);
    const char *message = JS_ToCString(ctx, exception);
    fprintf(stderr, "JavaScript exception: %s\n", message ? message : "<cannot format exception>");
    if (message)
        JS_FreeCString(ctx, message);
    else {
        JSValue formatting_exception = JS_GetException(ctx);
        JS_FreeValue(ctx, formatting_exception);
    }
    if (JS_IsError(exception)) {
        JSValue stack = JS_GetPropertyStr(ctx, exception, "stack");
        if (JS_IsException(stack)) {
            JSValue stack_exception = JS_GetException(ctx);
            JS_FreeValue(ctx, stack_exception);
        } else if (JS_IsString(stack)) {
            const char *trace = JS_ToCString(ctx, stack);
            if (trace) {
                fprintf(stderr, "%s\n", trace);
                JS_FreeCString(ctx, trace);
            } else {
                JSValue formatting_exception = JS_GetException(ctx);
                JS_FreeValue(ctx, formatting_exception);
            }
        }
        JS_FreeValue(ctx, stack);
    }
    JS_FreeValue(ctx, exception);
}

/* Strings are code units, not UTF-8: lone surrogates must survive the boundary. */
static int print_value(JSContext *ctx, JSValueConst value)
{
    if (JS_IsUndefined(value)) {
        puts("undefined");
    } else if (JS_IsNull(value)) {
        puts("null");
    } else if (JS_IsBool(value)) {
        printf("boolean\t%s\n", JS_ToBool(ctx, value) ? "true" : "false");
    } else if (JS_IsString(value)) {
        size_t length;
        const uint16_t *units = JS_ToCStringLenUTF16(ctx, &length, value);
        if (!units)
            return -1;
        fputs("string\t", stdout);
        for (size_t i = 0; i < length; i++)
            printf("%s%04x", i ? "," : "", (unsigned)units[i]);
        putchar('\n');
        JS_FreeCStringUTF16(ctx, units);
    } else if (JS_IsNumber(value) || JS_IsBigInt(value)) {
        const char *text = JS_ToCString(ctx, value);
        if (!text)
            return -1;
        if (JS_IsNumber(value)) {
            double number;
            if (JS_ToFloat64(ctx, &number, value) < 0) {
                JS_FreeCString(ctx, text);
                return -1;
            }
            printf("number\t%s\n", number == 0 && signbit(number) ? "-0" : text);
        } else {
            printf("bigint\t%s\n", text);
        }
        JS_FreeCString(ctx, text);
    } else {
        fputs("reference error: unsupported result type (expected primitive)\n", stderr);
        return 1;
    }
    return ferror(stdout) ? 1 : 0;
}

int main(int argc, char **argv)
{
    if (argc > 2 && !(argc == 3 && !strcmp(argv[1], "--source-name"))) {
        fprintf(stderr, "usage: %s [source.js|-] | --source-name NAME\n", argv[0]);
        return 2;
    }
    int stdin_named = argc == 3 && !strcmp(argv[1], "--source-name");
    const char *filename = stdin_named ? argv[2] : (argc == 2 && strcmp(argv[1], "-") ? argv[1] : "<stdin>");
    FILE *input = stdin;
    if (!stdin_named && strcmp(filename, "<stdin>")) {
        input = fopen(filename, "rb");
        if (!input) {
            fprintf(stderr, "reference error: cannot open %s: %s\n", filename, strerror(errno));
            return 2;
        }
    }
    size_t length;
    char *source = read_source(input, &length);
    if (input != stdin)
        fclose(input);
    if (!source) {
        fprintf(stderr, "reference error: cannot read %s: %s\n", filename, strerror(errno));
        return 2;
    }
    JSRuntime *runtime = JS_NewRuntime();
    if (!runtime) {
        fputs("reference error: cannot allocate runtime\n", stderr);
        free(source);
        return 2;
    }
    JSContext *ctx = JS_NewContext(runtime);
    if (!ctx) {
        fputs("reference error: cannot allocate context\n", stderr);
        JS_FreeRuntime(runtime);
        free(source);
        return 2;
    }
    struct execution_budget budget = {0};
    if (clock_gettime(CLOCK_MONOTONIC, &budget.start) != 0) {
        fputs("reference error: cannot initialize execution clock\n", stderr);
        JS_FreeContext(ctx);
        JS_FreeRuntime(runtime);
        free(source);
        return 2;
    }
    JS_SetInterruptHandler(runtime, interrupt_execution, &budget);
    JSValue value = JS_Eval(ctx, source, length, filename, JS_EVAL_TYPE_GLOBAL);
    free(source);
    int status;
    if (JS_IsException(value)) {
        if (budget.exceeded)
            fprintf(stderr, "reference error: execution budget exceeded (%d ms)\n", EXECUTION_BUDGET_MS);
        report_exception(ctx);
        status = 1;
    } else {
        status = print_value(ctx, value);
        if (status < 0) {
            report_exception(ctx);
            status = 1;
        }
    }
    JS_FreeValue(ctx, value);
    JS_FreeContext(ctx);
    JS_FreeRuntime(runtime);
    if (fflush(stdout) != 0) {
        fputs("reference error: cannot write result\n", stderr);
        status = 2;
    }
    return status;
}
