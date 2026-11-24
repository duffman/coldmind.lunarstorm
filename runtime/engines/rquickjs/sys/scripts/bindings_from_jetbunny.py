#!/usr/bin/env python3
# Copyright (c) 2026 Coldmind AB
# SPDX-License-Identifier: MIT
"""Write src/bindings/*.rs from jetbunny's binding files with one scripted change.

    bindings_from_jetbunny.py <jetbunny checkout>

jetbunny's bindings (crates/rquickjs-sys/src/bindings/) still define
`JS_ATOM_concat`, which the 0.17.0 quickjs-atom.h no longer has, so every atom
after it is one too high there. The change, applied identically to all 20
files: delete the `JS_ATOM_concat` constant and decrement every `JS_ATOM_*`
constant whose value was above it (JS_ATOM_END included). Nothing else in
jetbunny's text changes.

The result is checked by tests/bindings.rs against the atom order of
quickjs/quickjs-atom.h.
"""
import os
import re
import sys

ATOM = re.compile(r"^(pub const (JS_ATOM_\w+): \w+ = )(\d+);$")


def convert(text: str) -> str:
    """Drop JS_ATOM_concat and renumber the atoms after it.

    Args:
        text: one jetbunny binding file.

    Returns:
        The file with the change applied.

    Raises:
        SystemExit: the file has no JS_ATOM_concat (nothing to apply).
    """
    lines = text.split("\n")
    concat = None
    for line in lines:
        m = ATOM.match(line)
        if m and m.group(2) == "JS_ATOM_concat":
            concat = int(m.group(3))
    if concat is None:
        sys.exit("no JS_ATOM_concat: not a jetbunny binding file")
    out = []
    for line in lines:
        m = ATOM.match(line)
        if m and m.group(2) == "JS_ATOM_concat":
            continue
        if m and int(m.group(3)) > concat:
            line = f"{m.group(1)}{int(m.group(3)) - 1};"
        out.append(line)
    return "\n".join(out)


def main() -> None:
    """Convert every jetbunny binding file into src/bindings/."""
    source = os.path.join(sys.argv[1], "crates/rquickjs-sys/src/bindings")
    target = os.path.join(os.path.dirname(__file__), "..", "src", "bindings")
    for name in sorted(os.listdir(source)):
        if name.endswith(".rs"):
            with open(os.path.join(source, name)) as f:
                text = convert(f.read())
            with open(os.path.join(target, name), "w") as f:
                f.write(text)


if __name__ == "__main__":
    main()
