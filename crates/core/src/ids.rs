/// Declares a newtype wrapper around an `i64` row id so ids for different
/// entities (a `TaskId` and a `NoteId`, say) can't be mixed up at compile time.
macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub i64);

        impl From<i64> for $name {
            fn from(value: i64) -> Self {
                Self(value)
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

id_type!(ProjectId);
id_type!(RepoId);
id_type!(TaskId);
id_type!(NoteId);
id_type!(AttachmentId);
id_type!(CredentialId);
id_type!(CommitLinkId);
id_type!(RunCommandId);
