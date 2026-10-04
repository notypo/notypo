//! notypo: a Rust port of [thefuck](https://github.com/nvbn/thefuck), which
//! corrects errors in previous console commands.
//!
//! The module layout mirrors the Python package:
//!
//! | Rust | Python |
//! |---|---|
//! | [`app`] | `entrypoints/*` |
//! | [`args`] | `argument_parser.py` |
//! | [`settings`] | `conf.py`, `const.py` |
//! | [`shells`] | `shells/*` |
//! | [`output_readers`] | `output_readers/*` |
//! | [`types`], [`corrector`], [`ui`], [`logs`], [`utils`] | same names |
//! | [`specific`], [`rules`] | same names |
//!
//! [`difflib`] and [`shlex`] are exact ports of the Python stdlib functions
//! thefuck depends on; [`path_index`] caches the `$PATH` listing.

/// A lazily compiled, statically cached `regex::Regex`.
macro_rules! regex {
    ($re:expr $(,)?) => {{
        static RE: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new($re).unwrap());
        &*RE
    }};
}

pub mod app;
pub mod args;
pub mod corrector;
pub mod difflib;
pub mod engine;
pub mod logs;
pub mod output_readers;
pub mod path_index;
mod platform;
pub mod rules;
pub mod settings;
pub mod shell_logger;
pub mod shells;
pub mod shlex;
pub mod specific;
mod terminal;
pub mod types;
pub mod ui;
pub mod utils;

pub use app::run;
