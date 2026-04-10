use crate::config::model::ConfigV10;
use crate::target::{TargetRef, is_valid_alias};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Location {
    S3(TargetRef),
    Local(PathBuf),
}

pub fn parse_location(input: &str, config: &ConfigV10) -> Location {
    let normalized = input.trim().replace('\\', "/");
    let first = normalized.split('/').next().unwrap_or_default();

    if is_valid_alias(first) && config.aliases.contains_key(first) {
        if let Ok(target) = TargetRef::parse(input) {
            return Location::S3(target);
        }
    }

    Location::Local(PathBuf::from(input))
}
