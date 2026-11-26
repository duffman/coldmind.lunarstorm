/* Generate Rust metadata by letting the C preprocessor and compiler apply the
 * pinned upstream header's DEF/def rules. Keep the enumeration identical to
 * OPCodeEnum in quickjs.c; temporary IDs intentionally overlap short IDs. */
#include <stdio.h>
#include <ctype.h>

enum OPCodeEnum {
#define DEF(id, size, n_pop, n_push, f) OP_ ## id,
#define def(id, size, n_pop, n_push, f)
#include "quickjs-opcode.h"
    OP_COUNT,
    OP_TEMP_START = OP_nop + 1,
    OP___dummy = OP_TEMP_START - 1,
#define DEF(id, size, n_pop, n_push, f)
#define def(id, size, n_pop, n_push, f) OP_ ## id,
#include "quickjs-opcode.h"
    OP_TEMP_END,
};

_Static_assert(OP_COUNT <= 256, "final opcodes must fit in one byte");

static void rust_format(const char *name) {
    int uppercase = 1;
    for (; *name; name++) {
        if (*name == '_') {
            uppercase = 1;
        } else {
            putchar(uppercase ? toupper((unsigned char)*name) : *name);
            uppercase = 0;
        }
    }
}

enum Atoms { ATOM_ZERO,
#define DEF(id, str) ATOM_ ## id,
#include "quickjs-atom.h"
#undef DEF
ATOM_END };
int main(void) {
    printf("pub const ATOM_END: u32 = %d;\n", ATOM_END);
    printf("pub const BUILTIN_SYMBOL_START: u32 = %d;\n", ATOM_Private_brand);
    puts("pub static BUILTIN_ATOMS: &[&[u16]] = &[&[],");
#define DEF(id, str) { const unsigned char *p = (const unsigned char *)(str); fputs("&[", stdout); while (*p) printf("%u,", *p++); puts("],"); }
#include "quickjs-atom.h"
#undef DEF
    puts("];");
    puts("// Generated from the pinned vendor opcode and atom headers; do not edit.");
#ifndef SHARED_FORMATS
    puts("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub enum OperandFormat {");
#define FMT(f) rust_format(#f); puts(",");
#include "quickjs-opcode.h"
    puts("}");
#endif
    puts("/// Upstream OP_COUNT, including the invalid opcode at zero.");
    printf("pub const OPCODE_COUNT: usize = %d;\n", OP_COUNT);
    puts("/// First temporary ID; overlaps valid final short instructions, not a rejection threshold.");
    printf("pub const TEMPORARY_OPCODE_START: u8 = %d;\n", OP_TEMP_START);
    puts("/// Exclusive temporary ID endpoint; the temporary range overlaps final short instructions.");
    printf("pub const TEMPORARY_OPCODE_END: u16 = %d;\n", OP_TEMP_END);
    puts("/// Header-generated final identities, including invalid; no temporary aliases.");
    puts("#[allow(non_upper_case_globals)]\npub mod ids {");
#define DEF(id, size, n_pop, n_push, f) printf("pub const r#%s: u8 = %d;\n", #id, OP_ ## id);
#define def(id, size, n_pop, n_push, f)
#include "quickjs-opcode.h"
    puts("}");
    puts("/// All valid final regular and short opcodes in numeric order; invalid is excluded.");
    puts("pub static FINAL_OPCODES: &[Opcode] = &[");
#define DEF(id, size, n_pop, n_push, f) \
    if (OP_ ## id != OP_invalid) { \
        printf("Opcode { id: %d, name: \"%s\", width: %d, pop: %d, push: %d, format: OperandFormat::", \
               OP_ ## id, #id, size, n_pop, n_push); \
        rust_format(#f); \
        puts(" },"); \
    }
#define def(id, size, n_pop, n_push, f)
#include "quickjs-opcode.h"
    puts("];");
    return ferror(stdout) ? 1 : 0;
}
