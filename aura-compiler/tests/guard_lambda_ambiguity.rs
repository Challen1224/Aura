//! Match-arm guards vs lambda syntax: a guard ending in an identifier
//! (`if w == h =>`) or written parenthesized (`if (w == h) =>`) previously
//! mis-parsed — the parser read `h => ...` as a lambda (and `(w == h) =>`
//! as a lambda parameter list) and swallowed the arm's arrow. Guards now
//! parse with top-level lambda interpretation suppressed, cleared inside
//! any parentheses so lambda *arguments* in guards still work.

use aura_compiler::compile;

#[test]
fn guard_ending_in_identifier_parses() {
    let src = r#"
enum E { P(int a, int b) }
class Program {
    static void Main() {
        E e = E.P(2, 2);
        string s = match (e) {
            E.P(w, h) if w == h => "same",
            E.P(w, h) => "diff"
        };
        print(s);
    }
}
"#;
    compile(src, "test").expect("guard ending in identifier should parse");
}

#[test]
fn parenthesized_guard_parses() {
    let src = r#"
enum E { P(int a, int b) }
class Program {
    static void Main() {
        E e = E.P(2, 3);
        string s = match (e) {
            E.P(w, h) if (w == h) => "same",
            E.P(w, h) => "diff"
        };
        print(s);
    }
}
"#;
    compile(src, "test").expect("parenthesized guard should parse");
}

#[test]
fn lambda_argument_inside_guard_still_parses() {
    let src = r#"
class Program {
    static int Apply(Func<int, int> f, int v) { return f(v); }
    static void Main() {
        int n = 4;
        string s = match (n) {
            x if Program.Apply(y => y * 2, x) == 8 => "eight",
            * => "other"
        };
        print(s);
    }
}
"#;
    compile(src, "test").expect("lambda argument in guard should parse");
}

#[test]
fn lambdas_in_match_arm_bodies_still_parse() {
    let src = r#"
class Program {
    static int Apply(Func<int, int> f, int v) { return f(v); }
    static void Main() {
        int n = 1;
        int r = match (n) {
            1 => Program.Apply(x => x + 1, 1),
            * => 0
        };
        print(r);
    }
}
"#;
    compile(src, "test").expect("lambda in arm body should parse");
}
