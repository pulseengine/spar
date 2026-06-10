//! `spar-mcp` — standalone MCP stdio server binary.
//!
//! Reads JSON-RPC 2.0 messages from stdin (one per line) and writes
//! responses to stdout. The same loop is reachable via
//! `spar mcp serve`, which exec's this binary so the spar binary
//! itself does not need a build-time dependency on `spar-mcp` (which
//! would form a Cargo cycle through `spar-cli`).
//!
//! See the crate docs for the supported method surface.

fn main() {
    if let Err(e) = spar_mcp::server::run() {
        eprintln!("spar-mcp: stdio loop failed: {e}");
        std::process::exit(1);
    }
}
