"""check.py's own reading of Rust definitions, and its comparison with
graff's; each case holds an input that must make it say no.

    python3 evals/extract/test_check.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check as extract_check  # noqa: E402

SOURCE = """\
pub(crate) async unsafe fn load() {}
const fn first() {}
pub const MAX: usize = 3;
static mut COUNT: u32 = 0;
#[inline] fn short() {}
macro_rules! twice { () => {} }
    pub type Map = u32;
extern crate serde;
unsafe impl Send for Map {}
/// fn documented() {}
let fn_like = 1;
"""


def symbol(name, kind, start):
    return {"name": name, "kind": kind, "start": start}


def main():
    failed = total = 0

    def check(name, got, want):
        nonlocal failed, total
        total += 1
        if got != want:
            failed += 1
            print(f"FAIL {name}: {got!r} != {want!r}")

    check("expression: items after attributes, pub and qualifiers; not extern crate, impl, comments or lets",
          extract_check.by_expression(SOURCE),
          {(1, "fn", "load"), (2, "fn", "first"), (3, "const", "MAX"), (4, "static", "COUNT"), (5, "fn", "short"),
           (6, "macro_rules!", "twice"), (7, "type", "Map")})

    symbols = [symbol("load", "function", 1), symbol("first", "method", 2), symbol("MAX", "const", 3),
               symbol("COUNT", "static", 4), symbol("short", "function", 5), symbol("twice", "macro", 6),
               symbol("Map", "type_alias", 7), symbol("Map", "impl", 9)]
    matched, expression_only, graff_only = extract_check.compare(SOURCE, {"symbols": symbols})
    check("compare: the same definitions, an impl left out", (len(matched), expression_only, graff_only), (7, set(), set()))

    moved = [symbol("load", "function", 2), *symbols[1:], symbol("phantom", "struct", 11)]
    matched, expression_only, graff_only = extract_check.compare(SOURCE, {"symbols": moved})
    check("compare: a definition graff put on the wrong line, and one it invented",
          (len(matched), expression_only, graff_only),
          (6, {(1, "fn", "load")}, {(2, "fn", "load"), (11, "struct", "phantom")}))

    odd = "# [doc = \"x\"] struct ẕ0 {}\nconst A: &str = \"\x0b\";\r\nfn after() {}\n"
    check("expression: a spaced attribute, a Unicode name, and lines split at newlines only",
          extract_check.by_expression(odd), {(1, "struct", "ẕ0"), (2, "const", "A"), (3, "fn", "after")})
    opened = "thread_local! { static ref A: u32 = 0; }\ncrate::cfg_rt! { pub fn b() {} }\nmacro_rules! c {}\nformat!(\"fn d\");\n"
    check("expression: an item after a macro's opening, and lazy_static's static ref; not a string",
          extract_check.by_expression(opened), {(1, "static", "A"), (2, "fn", "b"), (3, "macro_rules!", "c")})

    macro = {"symbols": [symbol("twice", "macro", 6) | {"end": 9}], "syntax_error": False}
    check("why: a line inside a macro_rules body", extract_check.why(8, macro), "in a macro_rules body")
    check("why: the macro_rules line itself is no body", extract_check.why(6, macro), None)
    check("why: a line after it", extract_check.why(10, macro), None)
    check("why: a file with a syntax error", extract_check.why(10, macro | {"syntax_error": True}), "in a file with a syntax error")

    print(f"{total - failed}/{total} ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
