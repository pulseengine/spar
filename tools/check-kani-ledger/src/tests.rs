use super::*;

/// A mutated loop that never ends would otherwise hold the mutation job until
/// its 300 s per-mutant timeout (spar#641: 43 such mutants, about 54 minutes
/// on 4 jobs). The whole suite runs in about 0.1 s, so a process still alive
/// after 20 s has a loop that does not terminate, and that is a failure.
fn watchdog() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(20));
            eprintln!(
                "check-kani-ledger tests: still running after 20 s; a loop did not terminate"
            );
            std::process::abort();
        });
    });
}

#[test]
fn strip_blanks_comments_and_strings() {
    watchdog();
    let s = strip(
        "a // kani::assume(x)\nb /* kani::assume(y) */ \"kani::assume(z)\" r#\"kani::assume(w)\"# 'c' 'a",
    );
    assert!(!s.contains("assume"));
    assert_eq!(s.len(), "a // kani::assume(x)\nb /* kani::assume(y) */ \"kani::assume(z)\" r#\"kani::assume(w)\"# 'c' 'a".len());
    assert!(s.contains('\n'));
}

/// The expected strip output: every byte outside the kept fragments becomes a
/// space (newlines stay), so byte offsets are preserved as `strip` promises.
fn blanked(src: &str, keep: &[&str]) -> String {
    let mut out: Vec<u8> = src
        .bytes()
        .map(|c| if c == b'\n' { b'\n' } else { b' ' })
        .collect();
    let mut from = 0;
    for k in keep {
        let at = src[from..].find(k).expect("fragment") + from;
        out[at..at + k.len()].copy_from_slice(k.as_bytes());
        from = at + k.len();
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn strip_is_exact_on_every_literal_and_comment_form() {
    watchdog();
    let src = "a /* x /* y */ z */ b // c\nd \"e\\\"f\" g r#\"h\"i\"# j r\"k\" l '\\n' m 'n' o<'p> q b'r' s";
    let got = strip(src);
    let want = blanked(
        src,
        &["a", "b", "\n", "d", "g", "j", "l", "m", "o<'p>", "q b", "s"],
    );
    assert_eq!(got, want);
}

#[test]
fn strip_keeps_code_after_an_unterminated_form_untouched_before_it() {
    watchdog();
    // code before an unterminated block comment survives; the comment runs to EOF
    let src = "keep /* never closed\nstill comment";
    assert_eq!(strip(src), blanked(src, &["keep", "\n"]));
    let src = "x = r##\"a\"# still\"## y";
    assert_eq!(strip(src), blanked(src, &["x =", "y"]));
    let src = "ident_r\"s\" t";
    // `r` inside an identifier does not open a raw string; the plain string is blanked
    assert_eq!(strip(src), blanked(src, &["ident_r", "t"]));
}

#[test]
fn strip_handles_multibyte_char_literals() {
    watchdog();
    let src = "a 'é' b";
    assert_eq!(strip(src), blanked(src, &["a", "b"]));
}

#[test]
fn strip_edge_forms() {
    watchdog();
    // a raw identifier is code, not a raw string
    let src = "let r#type = 1; kani::assume(true);";
    assert_eq!(strip(src), src);
    // an unterminated string, char escape or quote at EOF does not panic
    for s in ["\"abc", "'\\", "x '", "r#\"open"] {
        let _ = strip(s);
    }
    // an escaped char literal ends at its own quote, with code right after it
    let src = "'\\n'm";
    assert_eq!(strip(src), blanked(src, &["m"]));
    // adjacent char literals
    let src = "('a','b')x";
    assert_eq!(strip(src), blanked(src, &["(", ",", ")x"]));
}

#[test]
fn multiplication_and_deref_are_code_not_comments() {
    watchdog();
    let src = "let a = x * y; let b = *p; let c = 2 *3;";
    assert_eq!(strip(src), src);
}
