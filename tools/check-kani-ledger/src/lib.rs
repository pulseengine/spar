//! Shrink-only ledger of the Kani assumptions that narrow a harness's input
//! range (REQ-PROOF-TRUST-ESCAPE-LEDGER-001).
//!
//! This part is the lexer step the scanner runs first: `strip` blanks
//! comments, string literals and char literals, so a pattern search sees code
//! only. The scanner and the ledger gate follow in the next two pull
//! requests (spar#641); the split keeps each one inside the 60 minute
//! in-diff mutation budget.

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

#[cfg(test)]
mod tests;
