pub mod classifier;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod error;
pub mod imap;
pub mod labels;
pub mod store;
pub mod summarizer;
pub(crate) mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = match option_env!("GIT_HASH") {
    Some(git_hash) => git_hash,
    None => "0000000000000000000000000000000000000000",
};
const BUILD_TIMESTAMP: &str = env!("GIT_TIMESTAMP");
