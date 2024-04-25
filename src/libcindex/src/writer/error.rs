use std::error::{self, Error};
use std::fmt;
use std::io;

/// The Error type for indexing operations.
///
/// Errors can come from std::io::Error, or
/// from indexing a file.
#[derive(Debug)]
pub struct IndexError {
    kind: IndexErrorKind,
    error: Box<dyn error::Error + Send + Sync>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexErrorKind {
    /// A read error returned from a std::io function
    IoError(io::ErrorKind),
    /// Indicates a filename isn't valid utf-8
    FileNameError,
    /// The file is longer than the specified max size
    FileTooLong,
    /// A line in the current file is longer than the specified max size
    LineTooLong,
    /// The number of trigrams in the read file exceeded the max number
    /// of trigrams
    TooManyTrigrams,
    /// Binary data is present in file (binary files are skipped)
    BinaryDataPresent,
    /// The ratio of invalid utf-8 : valid utf-8 chars is too high
    HighInvalidUtf8Ratio,
}

impl IndexError {
    /// Creates a new IndexError. Works the same as std::io::Error.
    ///
    /// ```
    /// # use libcindex::writer::{IndexResult, IndexError, IndexErrorKind};
    /// # use std::io::Write;
    /// // IndexError can be created from io::Error
    /// fn try_something() -> IndexResult<()> {
    ///     // like std::io::Error, IndexError can be created from strings
    ///     let custom_error = IndexError::new(IndexErrorKind::LineTooLong, "oh no!");
    ///     let mut b = Vec::<u8>::new();
    ///     // std::io::Error can be cast to IndexError in a try! macro
    ///     try!(b.write(b"some bytes"));
    ///     Ok(())
    /// }
    /// ```
    pub fn new<E>(kind: IndexErrorKind, error: E) -> IndexError
    where
        E: Into<Box<dyn error::Error + Send + Sync>>,
    {
        IndexError {
            kind: kind,
            error: error.into(),
        }
    }
    /// Returns the type of the error
    pub fn kind(&self) -> IndexErrorKind {
        self.kind.clone()
    }
}

impl From<io::Error> for IndexError {
    fn from(e: io::Error) -> Self {
        IndexError {
            kind: IndexErrorKind::IoError(e.kind()),
            error: Box::new(e),
        }
    }
}

impl From<IndexError> for io::Error {
    fn from(e: IndexError) -> Self {
        match e.kind() {
            IndexErrorKind::IoError(ekind) => io::Error::new(ekind, e),
            _ => io::Error::new(io::ErrorKind::Other, e),
        }
    }
}

impl Error for IndexError {}

impl fmt::Display for IndexError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        self.error.fmt(fmt)
    }
}

/// A specialized result type for Index operations.
///
/// Behaves similarly to std::io::Result
///
pub type IndexResult<T> = Result<T, IndexError>;
