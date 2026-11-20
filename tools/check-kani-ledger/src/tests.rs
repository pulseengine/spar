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

#[test]
fn direct_assume_is_attributed_to_its_fn() {
    watchdog();
    let (e, h) =
        scan_source("#[kani::proof]\nfn h() { let x: u8 = kani::any(); kani::assume(x < 3); }\n");
    assert_eq!(h, 1);
    assert_eq!(e, vec!["h assume".to_owned()]);
}

#[test]
fn unwind_and_stub_bind_to_the_next_fn() {
    watchdog();
    let (e, _) =
        scan_source("#[kani::proof]\n#[kani::unwind( 8 )]\n#[kani::stub(a::b, c::d)]\nfn h() {}\n");
    assert_eq!(
        e,
        vec!["h stub=a::b,c::d".to_owned(), "h unwind=8".to_owned()]
    );
}

#[test]
fn helper_call_sites_are_counted_through_two_levels() {
    watchdog();
    let src = "impl T { fn any() -> Self { kani::assume(true); T } }\n\
               fn twice() -> T { T::any() }\n\
               #[kani::proof]\nfn h() { let _ = twice(); let _ = T::any(); }\n";
    let (e, _) = scan_source(src);
    assert!(e.contains(&"T::any assume".to_owned()), "{e:?}");
    assert!(e.contains(&"twice via=T::any".to_owned()), "{e:?}");
    assert!(e.contains(&"h via=twice".to_owned()), "{e:?}");
    assert!(e.contains(&"h via=T::any".to_owned()), "{e:?}");
    assert_eq!(e.len(), 4, "{e:?}");
}

#[test]
fn kani_any_is_not_a_call_to_a_free_helper_named_any() {
    watchdog();
    let src = "fn any() -> u8 { kani::assume(true); 0 }\n#[kani::proof]\nfn h() { let x: u8 = kani::any(); let y = it.any(); }\n";
    let (e, _) = scan_source(src);
    assert_eq!(e, vec!["any assume".to_owned()], "{e:?}");
}

#[test]
fn macro_body_and_invocations_are_counted() {
    watchdog();
    let src = "fn band(lo: u64) { let p: u64 = kani::any(); kani::assume(p >= lo); }\n\
               macro_rules! band_proof { ($n:ident, $lo:expr) => { #[kani::proof] #[kani::unwind(2)] fn $n() { band($lo); } }; }\n\
               band_proof!(a, 1);\nband_proof!(b, 2);\n";
    let (e, _) = scan_source(src);
    assert!(e.contains(&"band assume".to_owned()), "{e:?}");
    assert!(e.contains(&"band_proof!::$n unwind=2".to_owned()), "{e:?}");
    assert!(e.contains(&"band_proof!::$n via=band".to_owned()), "{e:?}");
    assert_eq!(
        e.iter()
            .filter(|x| x.as_str() == "? via=band_proof!")
            .count(),
        2,
        "{e:?}"
    );
}

#[test]
fn a_body_is_found_past_brackets_where_clauses_and_bodiless_decls() {
    watchdog();
    let src = "trait T { fn g(); }\n\
               fn f(x: [u8; 2], y: (u8, u8)) -> u8 where u8: Copy { kani::assume(x[0] < 2); 0 }\n\
               fn h() { kani::assume(true); }\n";
    let (e, _) = scan_source(src);
    assert_eq!(
        e,
        vec!["f assume".to_owned(), "h assume".to_owned()],
        "{e:?}"
    );
}

#[test]
fn impl_type_is_the_self_type_without_generics_or_path() {
    watchdog();
    let src = "impl<T: Clone> Wrap<T> { fn any() -> u8 { kani::assume(true); 0 } }\n\
               impl kani::Arbitrary for m::Foo { fn any() -> Self { kani::assume(true); Foo } }\n\
               impl<T> Tr for Bar<T> where T: Copy { fn any() -> u8 { kani::assume(true); 0 } }\n";
    let (e, _) = scan_source(src);
    assert_eq!(
        e,
        vec![
            "Bar::any assume".to_owned(),
            "Foo::any assume".to_owned(),
            "Wrap::any assume".to_owned()
        ],
        "{e:?}"
    );
}

#[test]
fn the_innermost_fn_owns_an_occurrence() {
    watchdog();
    let src =
        "fn outer() { fn inner() { kani::assume(true); } kani::any_where(|v: &u8| *v < 2); }\n";
    let (e, _) = scan_source(src);
    assert_eq!(
        e,
        vec!["inner assume".to_owned(), "outer any_where".to_owned()],
        "{e:?}"
    );
}

#[test]
fn a_method_helper_is_matched_by_dot_call_and_an_assoc_fn_is_not() {
    watchdog();
    let src = "impl S { fn lim(&self) -> u8 { kani::assume(true); 0 } fn mk() -> S { kani::assume(true); S } }\n\
               #[kani::proof]\nfn h() { let s = S::mk(); s.lim(); let _ = x.mk(); }\n";
    let (e, _) = scan_source(src);
    assert_eq!(
        e,
        vec![
            "S::lim assume".to_owned(),
            "S::mk assume".to_owned(),
            "h via=S::lim".to_owned(),
            "h via=S::mk".to_owned()
        ],
        "{e:?}"
    );
}

#[test]
fn an_innocent_macro_is_not_counted() {
    watchdog();
    let src = "fn band() { kani::assume(true); }\n\
               macro_rules! noisy { () => { fn q() {} }; }\n\
               macro_rules! narrow { () => { fn r() { band(); } }; }\n\
               noisy!();\nnarrow!();\n";
    let (e, _) = scan_source(src);
    assert!(!e.iter().any(|x| x.contains("noisy")), "{e:?}");
    assert_eq!(
        e.iter().filter(|x| x.as_str() == "? via=narrow!").count(),
        1,
        "{e:?}"
    );
}

#[test]
fn edges_of_the_text_neither_panic_nor_miss() {
    watchdog();
    let (e, _) = scan_source("kani::assume(true)");
    assert_eq!(e, vec!["? assume".to_owned()]);
    for s in [
        "fn",
        "fn ",
        "macro_rules!",
        "impl",
        "#",
        "#[kani::unwind(",
        "kani::assume",
        "use kani::assume",
    ] {
        let _ = scan_source(s);
    }
    assert!(
        scan_source("kani::assumed(x); kani::assume_x(y); my_kani::assume(z)")
            .0
            .is_empty()
    );
    let (e, _) = scan_source("x.kani::assume (1)");
    assert_eq!(e.len(), 1, "{e:?}");
}

#[test]
fn use_forms_that_import_narrowing_are_counted() {
    watchdog();
    for (src, n) in [
        ("use kani::assume;", 1),
        ("use kani::{any, any_where};", 1),
        ("use kani::*;", 1),
        ("use kani::any;", 0),
        ("use std::assume;", 0),
        ("fn user() {}", 0),
    ] {
        let (e, _) = scan_source(src);
        assert_eq!(
            e.iter().filter(|x| x.ends_with(" import")).count(),
            n,
            "{src}: {e:?}"
        );
    }
}

#[test]
fn string_escapes_end_where_rust_ends_them() {
    watchdog();
    let src = "let a = \"x\\\\\"; kani::assume(true); let b = '\\''; kani::assume(false);";
    let (e, _) = scan_source(src);
    assert_eq!(
        e.iter().filter(|x| x.ends_with(" assume")).count(),
        2,
        "{e:?}"
    );
    let src = "let c = r#\"a\"#; kani::assume(true); let d = \"\\u{1F600}\"; kani::assume(true);";
    assert_eq!(scan_source(src).0.len(), 2);
}

#[test]
fn harness_count_and_attribute_binding() {
    watchdog();
    let src =
        "#[kani::proof]\nfn a() {}\n#[kani::proof]\n#[kani::unwind(3)]\nfn b() {}\nfn c() {}\n";
    let (e, h) = scan_source(src);
    assert_eq!(h, 2);
    assert_eq!(e, vec!["b unwind=3".to_owned()]);
}

#[test]
fn a_macro_ends_at_its_own_brace() {
    watchdog();
    let src = "macro_rules! m { () => {}; }\nfn h() { kani::assume(true); }\n";
    assert_eq!(scan_source(src).0, vec!["h assume".to_owned()]);
}

#[test]
fn a_bodiless_decl_owns_nothing_and_braces_in_brackets_are_not_the_body() {
    watchdog();
    let src = "trait T { fn g(); }\nconst _: () = { kani::assume(true); };\n";
    assert_eq!(scan_source(src).0, vec!["? assume".to_owned()]);
    let src = "fn f(x: [u8; { 2 }]) { kani::assume(true); }\n";
    assert_eq!(scan_source(src).0, vec!["f assume".to_owned()]);
}

#[test]
fn the_innermost_impl_names_the_type() {
    watchdog();
    let src = "impl A {\n fn x() {\n  impl B { fn any() -> u8 { kani::assume(true); 0 } }\n }\n fn y() {}\n fn z() {}\n fn w() {}\n}\n";
    assert_eq!(scan_source(src).0, vec!["B::any assume".to_owned()]);
}

#[test]
fn the_innermost_impl_wins_when_it_sits_at_the_end_of_the_outer_one() {
    watchdog();
    let src = "impl A {\n fn a() {}\n fn b() {}\n fn c() {}\n fn x() { impl B { fn any() -> u8 { kani::assume(true); 0 } } }\n}\n";
    assert_eq!(scan_source(src).0, vec!["B::any assume".to_owned()]);
}

#[test]
fn an_impl_whose_type_parses_empty_names_the_fn_alone() {
    watchdog();
    let (e, _) = scan_source(
        "impl <X> {\n#[kani::proof]\nfn h() { let x: u8 = kani::any(); kani::assume(x < 3); }\n}\n",
    );
    assert_eq!(e, vec!["h assume".to_owned()]);
}

#[test]
fn find_all_matches_a_text_of_exactly_the_pattern_length() {
    watchdog();
    assert_eq!(find_all(b"ab", "ab"), vec![0]);
    assert_eq!(find_all(b"a", "ab"), Vec::<usize>::new());
    assert_eq!(find_all(b"xab", "ab"), Vec::<usize>::new());
    assert_eq!(find_all(b"x.ab", ".ab"), vec![1]);
}

#[test]
fn compare_reports_growth_and_shrink_as_multisets() {
    watchdog();
    let f = vec!["p u assume".to_owned(), "p u assume".to_owned()];
    let l = vec!["p u assume".to_owned(), "p v assume".to_owned()];
    let (u, s) = compare(&f, &l);
    assert_eq!(u, vec!["p u assume".to_owned()]);
    assert_eq!(s, vec!["p v assume".to_owned()]);
}

#[test]
fn malformed_ledger_line_is_rejected() {
    watchdog();
    assert!(parse_ledger("a b c\n").is_err());
    assert!(parse_ledger("kernel\tf\tk\tn\n").is_err());
    assert_eq!(
        parse_ledger("# c\n\nsource\tf\tkani-assume\th \n").unwrap(),
        vec!["source\tf\tkani-assume\th".to_owned()]
    );
}

#[test]
fn every_canary_is_caught() {
    watchdog();
    let mut out = Vec::new();
    assert_eq!(
        negative_control(&mut out),
        0,
        "{}",
        String::from_utf8_lossy(&out)
    );
    assert_eq!(
        String::from_utf8_lossy(&out).matches("control ok").count(),
        canaries().len()
    );
}

#[test]
fn this_repository_matches_its_ledger() {
    watchdog();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    let code = run(&root, &root.join(LEDGER), &mut out);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
}

#[test]
fn negative_control_fails_on_a_missed_or_absent_control() {
    watchdog();
    assert_eq!(canaries().len(), 6);
    let mut out = Vec::new();
    let undetectable = vec![("no change", CANARY_BASE.to_owned())];
    assert_eq!(negative_control_with(&undetectable, &mut out), 1);
    assert!(String::from_utf8_lossy(&out).contains("CONTROL-MISSED no change"));
    let mut out = Vec::new();
    assert_eq!(negative_control_with(&[], &mut out), 1);
    let mut out = Vec::new();
    let two = vec![("no change", CANARY_BASE.to_owned()), canaries().remove(0)];
    assert_eq!(negative_control_with(&two, &mut out), 1);
    assert!(String::from_utf8_lossy(&out).contains("controls: 2 run, 1 missed"));
}

#[test]
fn walk_skips_hidden_target_node_modules_and_ci_controls() {
    watchdog();
    let d = std::env::temp_dir().join(format!("kani-ledger-walk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for sub in [
        "src",
        "target/x",
        ".hidden",
        "node_modules/m",
        "ci-controls/p",
        "a/target",
    ] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
        std::fs::write(d.join(sub).join("f.rs"), "").unwrap();
    }
    std::fs::write(d.join("src/g.txt"), "").unwrap();
    let got: Vec<String> = rs_files(&d)
        .unwrap()
        .iter()
        .map(|p| {
            p.strip_prefix(&d)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(got, vec!["src/f.rs".to_owned()]);
}
