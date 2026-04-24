#![no_main]
//! Fuzz target: feed arbitrary bytes to the AADL parser and assert it never
//! panics or hangs. Malformed input must be rejected cleanly via the error
//! recovery machinery — the parser has explicit recovery sets and this target
//! exercises those paths with adversarial input.
//!
//! Traceability: REQ-PARSE-001, REQ-PARSE-002, REQ-PARSER-001.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The parser consumes `&str`, so reject non-UTF8 silently — libfuzzer
    // will treat this as a trivial path, which is fine: the intent is
    // "no panic, no hang on malformed input", not "every byte sequence
    // reaches the parser".
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // Bound input length so a single iteration can't stall the fuzzer
    // on pathological O(n^k) grammar paths. `-timeout=5` on the libfuzzer
    // side is the real backstop; this is a soft cap for throughput.
    if text.len() > 64 * 1024 {
        return;
    }

    // Call the lossless parser. `Parse` owns the green tree and error list;
    // constructing a `SyntaxNode` and walking errors exercises the tree
    // builder, which is the other half of the parser stack.
    let parse = spar_syntax::parse(text);
    let _ = parse.syntax_node();
    let _ = parse.errors();
    let _ = parse.ok();
});
