//! The app's engine: a long-running process that owns the sample library,
//! so the app only does windows and UI.
//!
//!     samplary-engine serve <library.sqlite> [clap.onnx]
//!
//! serves the app over stdin and stdout (see `server`) until stdin closes,
//! then exits without waiting for a scan under way. With CLAP's model, it
//! listens to the samples as well as analyzing them.

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match &args[..] {
        [verb, db] if verb == "serve" => samplary_engine::server::run(Path::new(db), None),
        [verb, db, model] if verb == "serve" => {
            samplary_engine::server::run(Path::new(db), Some(Path::new(model)))
        }
        _ => Err("usage: samplary-engine serve <library.sqlite> [clap.onnx]".into()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
