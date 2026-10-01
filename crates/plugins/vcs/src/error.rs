//! What goes wrong in tau-vcs, named. The vcs tools hand these to the
//! model as their results, so each message says what to do next where
//! there is something to do.

use std::{io, path::PathBuf};

use jj_lib::{
    backend::BackendError,
    git::GitImportError,
    working_copy::CheckoutError,
    workspace::{WorkspaceInitError, WorkspaceLoadError},
};

use crate::clone::CloneError;

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("The description must not be empty")]
    EmptyDescription,
    #[error("The working copy has no parent")]
    NoParent,
    #[error("Name at least one path to restore (\".\" restores everything)")]
    NoPaths,
    #[error("The operation to undo is a merge; ask the user to undo it")]
    UndoMerge,
    #[error("There is nothing to undo")]
    NothingToUndo,
    #[error(
        "The operation log has concurrent operations here; ask the user to \
         undo from the operation log"
    )]
    ConcurrentOperations,
    #[error(
        "The last operation was not made by the vcs tools in this workspace \
         (\"{0}\"); vcs_undo only undoes its own operations"
    )]
    NotOurs(String),
    #[error("Bad undo record on operation")]
    BadUndoRecord,
    #[error("`{0}` is not a commit id")]
    NotCommitId(String),
    #[error("A commit id is not hex")]
    NotHex,
    #[error(
        "Your working copy has uncommitted changes ({0}). Commit your work \
         with vcs_commit first."
    )]
    Uncommitted(String),
    #[error("The parent's working copy is a merge")]
    ParentMerge,
    #[error("The child's changes do not form one stack")]
    NotOneStack,
    #[error("The workspace has no working-copy commit")]
    NoWorkingCopy,
    #[error("A landed change went missing")]
    LandedMissing,
    #[error("Change {0} is divergent after landing")]
    DivergentAfterLanding(String),
    #[error("No tau project at {}", .0.display())]
    NoProject(PathBuf),
    #[error("No tau project at {}: {source}", root.display())]
    ProjectLoad {
        root: PathBuf,
        source: WorkspaceLoadError,
    },
    #[error("`{0}` is not a change id")]
    NotChangeId(String),
    #[error("Change {change} of turn {turn} is divergent")]
    DivergentTurn { change: String, turn: u32 },
    #[error(
        "{} is not a Git repository. Only local repositories can be imported \
         for now",
        .0.display()
    )]
    NotGitRepo(PathBuf),
    #[error("{} is not a Git repository any more", .0.display())]
    NoLongerGitRepo(PathBuf),
    #[error("jj-lib panicked: {0}")]
    Panicked(String),
    #[error("The vcs thread has stopped")]
    Stopped,
    #[error("Cannot start the vcs thread: {0}")]
    Thread(#[source] io::Error),
    #[error("The working-copy commit {0} is immutable")]
    Immutable(String),
    #[error(
        "The working copy is stale: another process changed this workspace's \
         commit. Ask the user to update the workspace."
    )]
    Stale,
    #[error("Cannot lock the repository: {0}")]
    Lock(#[source] jj_lib::lock::FileLockError),
    #[error(
        "`{0}` is not a change id or a commit id. Pass an id (or a unique \
         prefix) from vcs_log or vcs_status; revsets are not accepted."
    )]
    NotAnId(String),
    #[error("No change matches `{0}`")]
    NoChange(String),
    #[error("Change id prefix `{0}` is ambiguous; give more of it")]
    AmbiguousChange(String),
    #[error("Change `{0}` is hidden (abandoned)")]
    Hidden(String),
    #[error("Change `{0}` is divergent; pass a commit id instead")]
    DivergentChange(String),
    #[error("No commit matches `{0}`")]
    NoCommit(String),
    #[error("Commit id prefix `{0}` is ambiguous; give more of it")]
    AmbiguousCommit(String),
    #[error("`{0}` is outside the repository")]
    OutsideRepo(String),
    #[error("`{0}` is not a path inside the repository")]
    NotInRepo(String),
    #[error("{commit} is not a commit id: {source}")]
    BadCommitHex {
        commit: String,
        source: gix::hash::decode::Error,
    },
    #[error("No Git store in {}: {source}", root.display())]
    NoGitStore {
        root: PathBuf,
        source: Box<gix::open::Error>,
    },
    #[error("No commit {hex}: {source}")]
    MissingCommit { hex: String, source: BackendError },
    #[error("No jj workspace at {}: {source}", root.display())]
    NoWorkspace {
        root: PathBuf,
        source: WorkspaceLoadError,
    },
    #[error("Cannot create {}: {source}", path.display())]
    Create { path: PathBuf, source: io::Error },
    #[error("Cannot delete {}: {source}", path.display())]
    Delete { path: PathBuf, source: io::Error },
    #[error("Cannot copy the Git store of {}: {source}", path.display())]
    CopyGitStore { path: PathBuf, source: io::Error },
    #[error("Cannot update from {}: {source}", path.display())]
    UpdateGitStore { path: PathBuf, source: io::Error },
    #[error("Cannot make the jj repository: {0}")]
    MakeRepo(#[source] WorkspaceInitError),
    #[error("Cannot add the workspace: {0}")]
    AddWorkspace(#[source] WorkspaceInitError),
    #[error("Cannot import the Git branches: {0}")]
    ImportBranches(#[source] GitImportError),
    #[error("Cannot check out the workspace's files: {0}")]
    CheckOut(#[source] CheckoutError),
    #[error("Cannot read {name}: {source}")]
    Read {
        name: String,
        source: jj_lib::diff_presentation::unified::UnifiedDiffError,
    },
    #[error(transparent)]
    Clone(#[from] CloneError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    CheckOutCommit(#[from] jj_lib::repo::CheckOutCommitError),
    #[error(transparent)]
    Checkout(#[from] jj_lib::working_copy::CheckoutError),
    #[error(transparent)]
    ConfigGet(#[from] jj_lib::config::ConfigGetError),
    #[error(transparent)]
    ConfigUpdate(#[from] jj_lib::config::ConfigUpdateError),
    #[error(transparent)]
    EditCommit(#[from] jj_lib::repo::EditCommitError),
    #[error(transparent)]
    OpStore(#[from] jj_lib::op_store::OpStoreError),
    #[error(transparent)]
    RepoLoader(#[from] jj_lib::repo::RepoLoaderError),
    #[error(transparent)]
    Snapshot(#[from] jj_lib::working_copy::SnapshotError),
    #[error(transparent)]
    TransactionCommit(#[from] jj_lib::transaction::TransactionCommitError),
    #[error(transparent)]
    WorkspaceInit(#[from] jj_lib::workspace::WorkspaceInitError),
    #[error(transparent)]
    Index(#[from] jj_lib::index::IndexError),
    #[error(transparent)]
    RewriteRoot(#[from] jj_lib::repo::RewriteRootCommit),
    #[error(transparent)]
    Revset(#[from] jj_lib::revset::RevsetEvaluationError),
    #[error(transparent)]
    WorkingCopyState(#[from] jj_lib::working_copy::WorkingCopyStateError),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    GitFind(#[from] gix::object::find::existing::Error),
    #[error(transparent)]
    GitCommit(#[from] gix::object::commit::Error),
    #[error(transparent)]
    GitTryInto(#[from] gix::object::try_into::Error),
}

impl From<VcsError> for tau_agent::tool::ToolError {
    fn from(error: VcsError) -> Self {
        Self::other(error)
    }
}

impl From<VcsError> for tau_agent::plugin::PluginError {
    fn from(error: VcsError) -> Self {
        Self::other(error)
    }
}
