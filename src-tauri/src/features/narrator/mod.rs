pub mod catalog;
pub mod dice;
mod generation;
mod injection;
pub mod model;
mod stream;
pub mod tools;
pub mod transcript;

pub use generation::{prepare, NarratorInputs};
pub use model::{Candidate, NarratorPurpose};
pub use stream::spawn;
