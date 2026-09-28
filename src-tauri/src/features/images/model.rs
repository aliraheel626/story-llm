pub use crate::features::transcript::model::StoryImage;

#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub description: String,
    pub character_ids: Vec<String>,
}
