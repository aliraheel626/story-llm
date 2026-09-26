pub mod catalog;
pub mod dice;
mod generation;
pub mod model;
mod stream;
pub mod tools;

pub use generation::{prepare, NarratorInputs};
pub use model::{Candidate, NarratorPurpose};
pub use stream::spawn;
