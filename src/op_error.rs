//! The error of an operation on pages and sections (move, rename, delete, restore), typed so that the
//! undo runner can tell a step that may work later from one that never will, and from one that
//! failed halfway. Existing callers that only show the text convert it into a `String` with `?`.

use std::fmt;

/// What a failed operation left behind and whether trying again can help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The operation is not possible as asked (a name it needs is taken); nothing changed.
    Refused,
    /// A file or folder is in use or locked; nothing changed and a later try can work.
    Busy,
    /// Any other failure; nothing changed.
    Failed,
    /// It failed after it had changed something (an item is partly moved or restored).
    Partial,
}

/// A failed operation: its `Kind` and the text for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpError {
    pub kind: Kind,
    pub text: String,
}

impl OpError {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        OpError { kind, text: text.into() }
    }

    pub fn refused(text: impl Into<String>) -> Self {
        Self::new(Kind::Refused, text)
    }

    pub fn busy(text: impl Into<String>) -> Self {
        Self::new(Kind::Busy, text)
    }

    /// The failure of a file system call: `Busy` when the file is in use, otherwise `Failed`.
    pub fn io(text: impl Into<String>, e: &std::io::Error) -> Self {
        Self::new(if is_in_use(e) { Kind::Busy } else { Kind::Failed }, text)
    }

    /// The same error, marked as one that changed something before it failed.
    pub fn partial(self) -> Self {
        Self::new(Kind::Partial, self.text)
    }
}

/// Whether `e` says that a file is open or locked by another program. Windows reports a folder that
/// holds an open file, or a file being scanned or deleted, as "access denied", so that counts too.
pub fn is_in_use(e: &std::io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    matches!(e.raw_os_error(), Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION))
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// A plain text error is a failure that changed nothing.
impl From<String> for OpError {
    fn from(text: String) -> Self {
        Self::new(Kind::Failed, text)
    }
}

impl From<&str> for OpError {
    fn from(text: &str) -> Self {
        Self::new(Kind::Failed, text)
    }
}

impl From<OpError> for String {
    fn from(e: OpError) -> Self {
        e.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_in_use_are_busy() {
        for code in [5, 32, 33] {
            assert_eq!(OpError::io("x", &std::io::Error::from_raw_os_error(code)).kind, Kind::Busy);
        }
        assert_eq!(OpError::io("x", &std::io::Error::from_raw_os_error(2)).kind, Kind::Failed);
        assert_eq!(OpError::io("x", &std::io::Error::other("y")).kind, Kind::Failed);
    }

    #[test]
    fn partial_keeps_the_text() {
        let e = OpError::busy("locked").partial();
        assert_eq!((e.kind, e.text.as_str()), (Kind::Partial, "locked"));
        assert_eq!(String::from(e), "locked");
    }
}
