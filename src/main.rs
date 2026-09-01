//! Binary entry point. Owns nothing: it hands the process straight to `app`
//! so the exit-code contract has exactly one implementation.

use devtrim::app;
use std::process::ExitCode;

fn main() -> ExitCode {
    app::main_impl()
}
