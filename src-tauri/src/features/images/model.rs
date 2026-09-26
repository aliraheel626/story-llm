pub use crate::features::ledger::model::StoryImage;

#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub description: String,
    pub character_ids: Vec<String>,
}
