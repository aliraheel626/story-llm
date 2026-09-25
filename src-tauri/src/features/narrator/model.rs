use crate::features::images::model::ImageRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NarratorPurpose {
    Action,
    Illustrate,
}

pub struct Candidate {
    pub visible: String,
    pub thoughts: Option<String>,
    pub image_requests: Vec<ImageRequest>,
}
