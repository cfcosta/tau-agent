/// Why a terminal call failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// libghostty-vt refused a call. Its API is pre-1.0; most of these
    /// are "cannot happen" states rather than bad input, since any byte
    /// stream is valid terminal input.
    #[error("libghostty-vt: {0}")]
    Vt(#[from] libghostty_vt::Error),
    /// Setting up the pseudo-terminal or the process failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
