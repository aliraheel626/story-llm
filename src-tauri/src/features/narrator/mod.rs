mod generation;
mod injection;
pub mod model;
pub mod staging;
mod stream;
pub mod tools;
pub mod transcript;

pub use generation::{prepare, NarratorInputs, Prepared};
pub use model::{Candidate, NarratorPurpose};
pub use stream::spawn;
