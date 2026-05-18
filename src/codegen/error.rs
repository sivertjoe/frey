use inkwell::builder::BuilderError;

#[derive(Debug)]
pub enum Error {
    Builder(BuilderError),
}

impl From<BuilderError> for Error {
    fn from(e: BuilderError) -> Self {
        Error::Builder(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Builder(e) => write!(f, "codegen error: {e}"),
        }
    }
}

impl std::error::Error for Error {}
