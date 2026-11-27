#[test]
fn pinned_oracle_preserves_boolean_local_values_mutation_and_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_boolean_locals/oracle.txt");
    for shape in [
        "scala.Boolean",
        "val flag",
        "var ",
        "while (",
        "unary_!",
        "&&",
        "||",
        "return",
        "tailSingleton",
        "tailConstant",
        "siblingScopes",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class BooleanLocals {{ {methods} }}"),
        "BooleanLocals.scala",
        SourceId::from_index(2477),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn immutable_boolean_locals_and_value_compositions_emit_complete_pairs() {
    for method in [
        "def f(x: Boolean): Boolean = { val flag: Boolean = x; flag }",
        "def f(x: Boolean): Boolean = { val flag = x; flag }",
        "def f(): Boolean = { val flag = true; flag }",
        "def f(x: Boolean): Boolean = { val flag = if x then false else true; flag }",
        "def f(x: Boolean, y: Boolean): Boolean = { val flag = x && y; flag }",
        "def f(x: Int): Boolean = { val flag = x > 0; flag }",
        "def f(x: Boolean): Boolean = { val flag = x match { case true => false; case _ => true }; flag }",
        "def f(x: Boolean): Int = { val flag = x; flag match { case true => 1; case _ => 2 } }",
        "def f(x: Boolean): Boolean = x match { case n => { val flag = n; flag } }",
        "def f(x: Boolean): Boolean = { val flag = x; (flag: Boolean) }",
        "def f(x: Boolean): Boolean = { val flag = x; return flag }",
        "def f(x: Boolean, y: Boolean): Boolean = { val flag = y; if x then { return true; flag } else false }",
        "def f(x: Boolean, n: Long): Long = { val flag = x; if flag then n else 0L }",
        "def f(x: Boolean, n: Float): Float = { val flag = x; if flag then n else 0.0f }",
        "def f(x: Boolean, n: Double): Double = { val flag = x; if flag then n else 0.0 }",
        "def f(x: Boolean): Int = if x then { val flag = true; if flag then 1 else 2 } else { val flag = false; if flag then 3 else 4 }",
        "def f(flag: Boolean): Boolean = { val flag = false; flag }",
        "def f(x: Boolean): Int = ({ x }) match { case true => 1; case _ => 2 }",
    ] {
        emit(method);
    }
}
