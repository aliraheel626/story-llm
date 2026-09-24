use std::sync::Arc;

use tokio::sync::Mutex;

use crate::features::images::model::ImageRequest;

use super::staging::TurnStaging;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NarratorPurpose {
    Action,
    Illustrate,
}

pub struct Candidate {
    pub visible: String,
    pub thoughts: Option<String>,
    pub staging: Option<Arc<Mutex<TurnStaging>>>,
    pub image_requests: Vec<ImageRequest>,
}
