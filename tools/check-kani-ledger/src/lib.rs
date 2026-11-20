//! Shrink-only ledger of the Kani assumptions that narrow a harness's input
//! range (REQ-PROOF-TRUST-ESCAPE-LEDGER-001).
//!
//! `strip` blanks comments, string literals and char literals, so a pattern
//! search sees code only. `scan_source` then reports each narrowing,
//! attributed to the enclosing fn (`Type::fn` inside an impl, `macro!` or
//! `macro!::fn` inside a `macro_rules!` body, `?` outside any unit):
//!
//! * `assume`      a `kani::assume(..)` call;
//! * `any_where`   a `kani::any_where(..)` call;
//! * `unwind=N`    a `#[kani::unwind(N)]` bound;
//! * `stub=A`      a `#[kani::stub(A)]` replacement;
//! * `import`      a `use kani::..` that brings `assume` or `any_where` into
//!   scope;
//! * `via=H`       a call to a same-file helper `H` that narrows (directly or
//!   through another helper).
//!
//! The ledger gate follows in the next pull request (spar#641); the split
//! keeps each one inside the 60 minute in-diff mutation budget.

use std::collections::BTreeSet;

/// Blank comments, string literals and char literals (keeping newlines and
/// byte offsets), so pattern search sees code only.
pub fn strip(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in out.iter_mut().take(to).skip(from) {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = b[i..]
                .iter()
                .position(|&x| x == b'\n')
                .map_or(b.len(), |p| i + p);
            blank(&mut out, i, end);
            i = end;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                    depth += 1;
                    j += 2;
                } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            blank(&mut out, i, j.min(b.len()));
            i = j;
        } else if c == b'r'
            && (i == 0 || !is_ident(b[i - 1]))
            && matches!(b.get(i + 1), Some(b'"') | Some(b'#'))
        {
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&b'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&b'"') {
                i += 1;
                continue;
            }
            j += 1;
            loop {
                if j >= b.len() {
                    break;
                }
                if b[j] == b'"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&b'#')) {
                    j += 1 + hashes;
                    break;
                }
                j += 1;
            }
            blank(&mut out, i, j.min(b.len()));
            i = j;
        } else if c == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let end = (j + 1).min(b.len());
            blank(&mut out, i, end);
            i = end;
        } else if c == b'\'' {
            // char literal 'x' or '\n' / '\u{..}'; a lifetime 'a has no closing quote
            if b.get(i + 1) == Some(&b'\\') {
                let end = b[i + 2..]
                    .iter()
                    .position(|&x| x == b'\'')
                    .map_or(b.len(), |p| i + 2 + p + 1);
                blank(&mut out, i, end);
                i = end;
            } else if let Some(n) = src[i + 1..].chars().next() {
                let after = i + 1 + n.len_utf8();
                if b.get(after) == Some(&b'\'') {
                    blank(&mut out, i, after + 1);
                    i = after + 1;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn ident_at(s: &[u8], mut i: usize) -> (String, usize) {
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    let st = i;
    while i < s.len() && (is_ident(s[i]) || s[i] == b'$') {
        i += 1;
    }
    (String::from_utf8_lossy(&s[st..i]).into_owned(), i)
}

/// Index of the brace closing the one at `open`, or `len` if unbalanced.
fn close_of(s: &[u8], open: usize) -> usize {
    let mut d = 0usize;
    for (k, &c) in s.iter().enumerate().skip(open) {
        if c == b'{' {
            d += 1;
        } else if c == b'}' {
            d -= 1;
            if d == 0 {
                return k;
            }
        }
    }
    s.len()
}

fn word_at(s: &[u8], i: usize, w: &str) -> bool {
    s[i..].starts_with(w.as_bytes())
        && (i == 0 || !is_ident(s[i - 1]))
        && s.get(i + w.len()).is_none_or(|&c| !is_ident(c))
}

#[derive(Debug, Clone)]
struct Unit {
    name: String,
    start: usize,
    end: usize,
    is_macro: bool,
    takes_self: bool,
    impl_ty: Option<String>,
    harness: bool,
}

fn units(s: &[u8]) -> Vec<Unit> {
    let mut impls: Vec<(usize, usize, String)> = Vec::new();
    let mut out = Vec::new();
    let mut i = 0;
    while s.get(i).is_some() {
        if word_at(s, i, "impl") {
            if let Some(off) = s[i..].iter().position(|&c| c == b'{' || c == b';') {
                let o = i + off;
                if s[o] == b'{' {
                    let head = String::from_utf8_lossy(&s[i + 4..o]).into_owned();
                    let head = head.rsplit(" for ").next().unwrap_or("").to_owned();
                    let mut depth = 0i32;
                    let no_generics: String = head
                        .chars()
                        .filter(|&c| {
                            if c == '<' {
                                depth += 1;
                            } else if c == '>' {
                                depth -= 1;
                                return false;
                            }
                            depth == 0
                        })
                        .collect();
                    let ty = no_generics
                        .split("where")
                        .next()
                        .unwrap_or("")
                        .trim()
                        .rsplit("::")
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_owned();
                    impls.push((o, close_of(s, o), ty));
                }
            }
            i += 4;
        } else if word_at(s, i, "macro_rules") {
            let mut j = i + "macro_rules".len();
            while j < s.len() && (s[j].is_ascii_whitespace() || s[j] == b'!') {
                j += 1;
            }
            let (name, k) = ident_at(s, j);
            if let Some(off) = s[k..].iter().position(|&c| c == b'{') {
                let o = k + off;
                debug_assert_eq!(s[o], b'{');
                out.push(Unit {
                    name: format!("{name}!"),
                    start: i,
                    end: close_of(s, o),
                    is_macro: true,
                    takes_self: false,
                    impl_ty: None,
                    harness: false,
                });
            }
            i = k;
        } else if word_at(s, i, "fn") {
            let (name, k) = ident_at(s, i + 2);
            // body: first `{` at paren depth 0 before any `;`
            let mut j = k;
            let mut paren = 0i32;
            let mut body = None;
            while j < s.len() {
                match s[j] {
                    b'(' | b'[' => paren += 1,
                    b')' | b']' => paren -= 1,
                    b';' if paren == 0 => break,
                    b'{' if paren == 0 => {
                        body = Some(j);
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
            if let (false, Some(o)) = (name.is_empty(), body) {
                let sig = &s[k..o];
                let takes_self = (0..sig.len()).any(|p| word_at(sig, p, "self"));
                let impl_ty = impls
                    .iter()
                    .filter(|(a, b, _)| (*a..*b).contains(&i))
                    // the innermost enclosing impl is the one that opens last
                    .max_by_key(|(a, _, _)| *a)
                    .map(|x| x.2.clone());
                out.push(Unit {
                    name,
                    start: i,
                    end: close_of(s, o),
                    is_macro: false,
                    takes_self,
                    impl_ty,
                    harness: false,
                });
            }
            i = k;
        } else {
            i += 1;
        }
    }
    out
}

/// The innermost unit containing `pos`, rendered as an attribution string.
fn owner(units: &[Unit], pos: usize) -> (Option<usize>, String) {
    let inner = units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.start <= pos && pos <= u.end)
        .min_by_key(|(_, u)| u.end - u.start);
    let mac = units
        .iter()
        .filter(|u| u.is_macro && (u.start..=u.end).contains(&pos))
        .max_by_key(|u| u.start);
    match inner {
        None => (None, "?".to_owned()),
        Some((k, u)) if u.is_macro => (Some(k), u.name.clone()),
        Some((k, u)) => {
            let base = match &u.impl_ty {
                Some(t) if !t.is_empty() => format!("{t}::{}", u.name),
                _ => u.name.clone(),
            };
            match mac {
                Some(m) => (Some(k), format!("{}::{base}", m.name)),
                None => (Some(k), base),
            }
        }
    }
}

fn find_all(hay: &[u8], pat: &str) -> Vec<usize> {
    let p = pat.as_bytes();
    if p.is_empty() || hay.len() < p.len() {
        return Vec::new();
    }
    (0..=hay.len() - p.len())
        .filter(|&i| &hay[i..i + p.len()] == p && (i == 0 || !is_ident(hay[i - 1]) || p[0] == b'.'))
        .collect()
}

/// Text after `pos` up to the paren closing the first `(`, whitespace removed.
fn paren_arg(s: &[u8], pos: usize) -> String {
    let Some(off) = s[pos..].iter().position(|&c| c == b'(') else {
        return String::new();
    };
    let o = pos + off;
    let mut d = 0i32;
    for (k, &c) in s.iter().enumerate().skip(o) {
        if c == b'(' {
            d += 1;
        } else if c == b')' {
            d -= 1;
            if d == 0 {
                return String::from_utf8_lossy(&s[o + 1..k])
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
            }
        }
    }
    String::new()
}

/// Ledger entries (`<unit> <kind>`, path added by the caller) and the number of
/// `#[kani::proof]` harness attributes in one file.
pub fn scan_source(src: &str) -> (Vec<String>, usize) {
    let st = strip(src);
    let s = st.as_bytes();
    let mut us = units(s);
    let squash: String = st.chars().filter(|c| !c.is_whitespace()).collect();
    let harnesses = squash.matches("#[kani::proof]").count();

    // harness attribution: the next fn after each #[kani::proof]
    let mut direct: Vec<(usize, String)> = Vec::new(); // (pos, kind)
    for pos in find_all(s, "#") {
        let attr: String = s[pos..]
            .iter()
            .take_while(|&&c| c != b']')
            .map(|&c| c as char)
            .filter(|c| !c.is_whitespace())
            .collect();
        let next_fn = us
            .iter()
            .enumerate()
            .filter(|(_, u)| !u.is_macro && u.start.cmp(&pos).is_gt())
            .min_by_key(|(_, u)| u.start)
            .map(|(k, u)| (k, u.start));
        let Some((k, at)) = next_fn else { continue };
        if attr.starts_with("#[kani::proof") {
            us[k].harness = true;
        } else if attr.starts_with("#[kani::unwind(") {
            direct.push((at, format!("unwind={}", paren_arg(s, pos))));
        } else if attr.starts_with("#[kani::stub(") {
            direct.push((at, format!("stub={}", paren_arg(s, pos))));
        }
    }
    for pat in ["kani::assume", "kani::any_where"] {
        for pos in find_all(s, pat) {
            let after = pos + pat.len();
            if s.get(after).is_some_and(|&c| is_ident(c)) {
                continue;
            }
            let rest = &s[after..];
            let nonws = rest
                .iter()
                .position(|c| !c.is_ascii_whitespace())
                .map(|p| rest[p]);
            if nonws == Some(b'(') {
                direct.push((pos, pat.trim_start_matches("kani::").to_owned()));
            }
        }
    }
    for pos in find_all(s, "use") {
        if !word_at(s, pos, "use") {
            continue;
        }
        let end = s[pos..]
            .iter()
            .position(|&c| c == b';')
            .map_or(s.len(), |p| pos + p);
        let stmt: String = String::from_utf8_lossy(&s[pos..end])
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if stmt.contains("kani::")
            && (stmt.contains("assume") || stmt.contains("any_where") || stmt.ends_with("::*"))
        {
            direct.push((pos, "import".to_owned()));
        }
    }

    let mut entries: Vec<String> = Vec::new();
    let mut narrowing: BTreeSet<usize> = BTreeSet::new();
    for (pos, kind) in &direct {
        let (k, name) = owner(&us, *pos);
        entries.push(format!("{name} {kind}"));
        if let Some(k) = k {
            mark(&us, &mut narrowing, k);
        }
    }

    // helpers: non-harness units that narrow; count each call site, to a fixpoint
    let mut counted: BTreeSet<(usize, usize)> = BTreeSet::new();
    loop {
        let helpers: Vec<usize> = narrowing
            .iter()
            .copied()
            .filter(|&k| !us[k].harness)
            .collect();
        let before = narrowing.len();
        for h in helpers {
            let u = &us[h];
            let mut pats: Vec<String> = Vec::new();
            if u.is_macro {
                pats.push(u.name.clone()); // `name!`
            } else if let Some(t) = u.impl_ty.as_ref().filter(|t| !t.is_empty()) {
                pats.push(format!("{t}::{}(", u.name));
                pats.push(format!("Self::{}(", u.name));
                if u.takes_self {
                    pats.push(format!(".{}(", u.name));
                }
            } else {
                pats.push(format!("{}(", u.name));
            }
            let label = owner(&us, u.start).1;
            for p in &pats {
                for pos in find_all(s, p) {
                    // skip the definition itself and qualified calls to another item (`kani::any(`)
                    let before_txt =
                        String::from_utf8_lossy(&s[pos.saturating_sub(4)..pos]).into_owned();
                    if before_txt.trim_end().ends_with("fn") {
                        continue;
                    }
                    if !u.is_macro
                        && u.impl_ty.is_none()
                        && pos >= 2
                        && (&s[pos - 2..pos] == b"::" || s[pos - 1] == b'.')
                    {
                        continue;
                    }
                    if u.is_macro && (u.start..=u.end).contains(&pos) {
                        continue;
                    }
                    if !counted.insert((h, pos)) {
                        continue;
                    }
                    let (k, name) = owner(&us, pos);
                    entries.push(format!("{name} via={label}"));
                    if let Some(k) = k {
                        mark(&us, &mut narrowing, k);
                    }
                }
            }
        }
        if narrowing.len() == before {
            break;
        }
    }
    entries.sort();
    (entries, harnesses)
}

/// Mark unit `k` as narrowing, and every `macro_rules!` whose body holds it:
/// each invocation of such a macro expands a narrowing item.
fn mark(us: &[Unit], set: &mut BTreeSet<usize>, k: usize) {
    set.insert(k);
    let at = us[k].start;
    for (m, u) in us.iter().enumerate() {
        if u.is_macro && m != k && u.start <= at && at <= u.end {
            set.insert(m);
        }
    }
}

#[cfg(test)]
mod tests;
