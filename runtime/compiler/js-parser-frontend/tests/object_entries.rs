use qjs_js_parser_frontend::{SourceMode, compile_program};
#[test]
fn computed_keys_accessors_prototype_and_object_spread_are_supported() {
    for source in [
        "({[key()]:value()});",
        "({get value(){return 42;},set value(x){this.saved=x;}});",
        "({get [key()](){return 42;},set [key()](value){this.saved=value;}});",
        "({__proto__:prototype});",
        "({...source,answer:42});",
        "({1e3:42,123n:1});",
    ] {
        compile_program(source, "object.js", SourceMode::Script).unwrap();
    }
}
