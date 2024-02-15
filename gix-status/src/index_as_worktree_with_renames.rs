//! Changes between the index and the worktree along with optional rename tracking.
mod types {
    use crate::index_as_worktree::EntryStatus;
    use bstr::BStr;
    use std::sync::atomic::AtomicBool;

    /// The error returned by [index_as_worktree_with_renames()`](crate::index_as_worktree_with_renames()).
    #[derive(Debug, thiserror::Error)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error(transparent)]
        TrackedFileModifications(#[from] crate::index_as_worktree::Error),
        #[error(transparent)]
        DirWalk(#[from] gix_dir::walk::Error),
    }

    /// Options for use in [index_as_worktree_with_renames()](super::).
    #[derive(Clone, Default)]
    pub struct Options {
        /// Options to configure how modifications to tracked files should be obtained.
        pub tracked_file_modifications: crate::index_as_worktree::Options,
        /// Options to control the directory walk that informs about untracked files.
        ///
        /// Note that we forcefully disable emission of tracked files to avoid any overlap
        /// between emissions to indicate modifications, and those that are obtained by
        /// the directory walk.
        ///
        /// If `None`, the directory walk portion will not run at all, yielding data similar
        /// to a bare [index_as_worktree()](crate::index_as_worktree()) call.
        pub dirwalk: Option<gix_dir::walk::Options>,
    }

    /// Provide additional information collected during the runtime of [`index_as_worktree_with_renames()`](crate::index_as_worktree_with_renames()).
    #[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
    pub struct Outcome {
        /// The outcome of the modification check of tracked files.
        tracked_file_modification: crate::index_as_worktree::Outcome,
        /// The outcome of the directory walk.
        dirwalk: gix_dir::walk::Outcome,
    }

    /// An 'entry' in the sense of a merge of modified tracked files and results from a directory walk.
    #[derive(Clone, PartialEq, Debug)]
    pub enum Entry<'a, ContentChange, SubmoduleStatus> {
        /// A tracked file was modified, and index-specific information is passed.
        Modification {
            /// All entries in the index.
            entries: &'a [gix_index::Entry],
            /// The entry with modifications.
            entry: &'a gix_index::Entry,
            /// The index of the `entry` for lookup in `entries` - useful to look at neighbors.
            entry_index: usize,
            /// The repository-relative path of the entry.
            rela_path: &'a BStr,
            /// The computed status of the entry.
            status: EntryStatus<ContentChange, SubmoduleStatus>,
        },
        /// An entry returned by the directory walk, without any relation to the index.
        ///
        /// This can happen if ignored files are returned as well, or if rename-tracking is disabled.
        DirectoryContents {
            /// The entry found during the disk traversal.
            entry: gix_dir::EntryRef<'a>,
            /// `collapsed_directory_status` is `Some(dir_status)` if this `entry` was part of a directory with the given
            /// `dir_status` that wasn't the same as the one of `entry` and if [gix_dir::walk::Options::emit_collapsed] was
            /// [CollapsedEntriesEmissionMode::OnStatusMismatch](gix_dir::walk::CollapsedEntriesEmissionMode::OnStatusMismatch).
            /// It will also be `Some(dir_status)` if that option was [CollapsedEntriesEmissionMode::All](gix_dir::walk::CollapsedEntriesEmissionMode::All).
            collapsed_directory_status: Option<gix_dir::entry::Status>,
        },
        /// The rewrite tracking discovered a match between a deleted and added file, and considers them equal enough,
        /// depending on the tracker settings.
        ///
        /// Note that the source of the rewrite is always the index as it detects the absence of entries, something that
        /// can't be done during a directory walk.
        Rewrite {
            /// All entries in the index.
            index_entries: &'a [gix_index::Entry],
            /// The entry that is the source of the rewrite, which means it was removed on disk,
            /// equivalent to [Change::Removed](crate::index_as_worktree::Change::Removed).
            ///
            /// Note that the [entry-id](gix_index::Entry::id) is the content-id of the source of the rewrite.
            source_entry: &'a gix_index::Entry,
            /// The index of the `source_entry` for lookup in `index_entries` - useful to look at neighbors.
            source_entry_index: usize,
            /// The repository-relative path of the `source_entry`.
            source_rela_path: &'a BStr,
            /// The computed status of the `source_entry`, which would always be
            source_status: EntryStatus<ContentChange, SubmoduleStatus>,

            /// The untracked entry found during the disk traversal, the destination of the rewrite.
            ///
            /// Note that its [`rela_path`](gix_dir::EntryRef::rela_path) is the destination of the rewrite, and the current
            /// location of the entry.
            dirwalk_entry: gix_dir::EntryRef<'a>,
            /// `collapsed_directory_status` is `Some(dir_status)` if this `dirwalk_entry` was part of a directory with the given
            /// `dir_status` that wasn't the same as the one of `entry` and if [gix_dir::walk::Options::emit_collapsed] was
            /// [CollapsedEntriesEmissionMode::OnStatusMismatch](gix_dir::walk::CollapsedEntriesEmissionMode::OnStatusMismatch).
            /// It will also be `Some(dir_status)` if that option was [CollapsedEntriesEmissionMode::All](gix_dir::walk::CollapsedEntriesEmissionMode::All).
            dirwalk_entry_collapsed_directory_status: Option<gix_dir::entry::Status>,
            /// The object id after the rename, specifically hashed in order to determine equality.
            dirwalk_entry_id: gix_hash::ObjectId,
            /// It's `None` if `source_entry.id` is equal to `dirwalk_entry_id`, as identity made an actual diff computation unnecessary.
            /// Otherwise, and if enabled, it's `Some(stats)` to indicate how similar both entries were.
            diff: Option<gix_diff::blob::DiffLineStats>,

            /// If true, this rewrite is created by copy, and `source_entry.id` is pointing to its source.
            /// Otherwise it's a rename, and `source_entry.id` points to a deleted object,
            /// as renames are tracked as deletions and additions of the same or similar content.
            copy: bool,
        },
    }

    /// The context for [index_as_worktree_with_renames()`](crate::index_as_worktree_with_renames()).
    pub struct Context<'a> {
        /// The pathspec to limit the amount of paths that are checked. Can be empty to allow all paths.
        ///
        /// Note that these are expected to have a [commont_prefix()](gix_pathspec::Search::common_prefix()) according
        /// to the prefix of the repository to efficiently limit the scope of the paths we process, both for the
        /// index modifications as well as for the
        pub pathspec: gix_pathspec::Search,
        /// A stack pre-configured to allow accessing attributes for each entry, as required for `filter`
        /// and possibly pathspecs.
        /// It must also allow accessing `.gitignore` information for use in the directory walk.
        pub attr_and_ignore_stack: gix_worktree::Stack,
        /// A filter to be able to perform conversions from and to the worktree format.
        ///
        /// It is needed to potentially refresh the index with data read from the worktree, which needs to be converted back
        /// to the form stored in Git.
        ///
        /// Note that for this to be correct, the attribute `stack` must be configured correctly as well.
        pub filter: gix_filter::Pipeline,
        /// A flag to query to learn if cancellation is requested.
        pub should_interrupt: &'a AtomicBool,
        /// The context for the directory walk.
        pub dirwalk: DirwalkContext<'a>,
    }

    /// All information that is required to perform a [dirwalk](gix_dir::walk()).
    pub struct DirwalkContext<'a> {
        /// The `git_dir` of the parent repository, after a call to [`gix_path::realpath()`].
        ///
        /// It's used to help us differentiate our own `.git` directory from nested unrelated repositories,
        /// which is needed if `core.worktree` is used to nest the `.git` directory deeper within.
        pub git_dir_realpath: &'a std::path::Path,
        /// The current working directory as returned by `gix_fs::current_dir()` to assure it respects `core.precomposeUnicode`.
        /// It's used to produce the realpath of the git-dir of a repository candidate to assure it's not our own repository.
        pub current_dir: &'a std::path::Path,
        /// A utility to lookup index entries faster, and deal with ignore-case handling.
        ///
        /// Must be set if `ignore_case` is `true`, or else some entries won't be found if their case is different.
        ///
        /// [Read more in `gix-dir`](gix_dir::walk::Context::ignore_case_index_lookup).
        pub ignore_case_index_lookup: Option<&'a gix_index::AccelerateLookup<'a>>,
    }

    /// Observe the status of an entry by comparing an index entry to the worktree, along
    /// with potential directory walk results.
    pub trait VisitEntry<'a> {
        /// Data generated by comparing an entry with a file.
        type ContentChange;
        /// Data obtained when checking the submodule status.
        type SubmoduleStatus;
        /// Observe the `status` of `entry` at the repository-relative `rela_path` at `entry_index`
        /// (for accessing `entry` and surrounding in the complete list of `entries`).
        fn visit_entry(&mut self, entry: Entry<'a, Self::ContentChange, Self::SubmoduleStatus>);
    }
}
pub use types::{Context, DirwalkContext, Entry, Error, Options, Outcome, VisitEntry};

#[allow(dead_code, unused_variables)]
pub(super) mod function {
    use crate::index_as_worktree::traits::{CompareBlobs, SubmoduleStatus};
    use crate::index_as_worktree_with_renames::{Context, Error, Options, Outcome, VisitEntry};
    use std::path::Path;

    /// Similar to [`index_as_worktree(…)`](crate::index_as_worktree()), except that it will automatically
    /// track renames if enabled, while additionally providing information about untracked files
    /// (or more, depending on the configuration).
    #[allow(clippy::too_many_arguments)]
    pub fn index_as_worktree_with_renames<'index, T, U, Find, E>(
        index: &'index gix_index::State,
        worktree: &Path,
        collector: &mut impl VisitEntry<'index, ContentChange = T, SubmoduleStatus = U>,
        compare: impl CompareBlobs<Output = T> + Send + Clone,
        submodule: impl SubmoduleStatus<Output = U, Error = E> + Send + Clone,
        objects: Find,
        progress: &mut dyn gix_features::progress::Progress,
        ctx: Context<'_>,
        options: Options,
    ) -> Result<Outcome, Error>
    where
        T: Send,
        U: Send,
        E: std::error::Error + Send + Sync + 'static,
        Find: gix_object::Find + Send + Clone,
    {
        todo!()
    }
}
