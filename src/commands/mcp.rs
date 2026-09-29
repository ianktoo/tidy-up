//! `tidy-up mcp`
//!
//! Serves the Model Context Protocol on stdin and stdout so an agent can drive
//! tidy-up. The protocol itself lives in [`crate::mcp`]; this resolves the
//! root, applies the guard once at startup, and hands over the streams.

use anyhow::Result;

use crate::{
    api::{ErrorCode, Outcome, Refused},
    cli::McpArgs,
    commands::guard::Guard,
    fsops::resolve_root,
    mcp::{Server, serve},
    safety::{Risk, assess},
    ui,
};

/// Starts the server and runs until stdin closes.
pub fn run(args: &McpArgs) -> Result<Outcome> {
    // stdout is the protocol channel. Anything else printed on it would be
    // read as a malformed message, so prose is silenced before anything runs.
    ui::set_quiet(true);

    let root = resolve_root(&args.root)?;

    // The guard runs once, here, rather than per request. A server rooted at a
    // folder the guard refuses should not start at all: an agent holding a
    // connection to it would otherwise find every call failing, and there is
    // no way to pass the override through this interface by design.
    let verdict = assess(&root);
    if verdict.risk == Risk::Dangerous {
        return Err(Refused::at(
            ErrorCode::SystemFolder,
            &root,
            format!(
                "refusing to serve {}: {} There is no way to override this over MCP.",
                root.display(),
                verdict.headline()
            ),
        )
        .into());
    }
    if args.allow_writes {
        // Writes are about to be exposed, so confirm the folder is writable
        // now rather than letting the first apply_plan discover it.
        Guard::read().check(&args.root, &root)?;
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = Server::new(root, args.allow_writes);
    serve(&mut server, stdin.lock(), stdout.lock())?;
    Ok(Outcome::default())
}
