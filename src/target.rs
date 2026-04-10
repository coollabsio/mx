use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRef {
    pub alias: String,
    pub bucket: Option<String>,
    pub key: Option<String>,
    pub trailing_slash: bool,
}

impl TargetRef {
    pub fn parse(input: &str) -> Result<Self> {
        let raw = input.trim();
        if raw.is_empty() {
            bail!("Target cannot be empty.");
        }

        let normalized = raw.replace('\\', "/");
        let trailing_slash = normalized.ends_with('/');
        let trimmed = normalized.trim_matches('/');
        if trimmed.is_empty() {
            bail!("Target cannot be empty.");
        }

        let mut parts = trimmed.splitn(3, '/');
        let alias = parts.next().unwrap_or_default().to_string();
        validate_alias(&alias)?;

        let bucket = parts.next().map(str::to_string);
        let key = parts
            .next()
            .map(str::to_string)
            .and_then(|value| if value.is_empty() { None } else { Some(value) });

        Ok(Self {
            alias,
            bucket,
            key,
            trailing_slash,
        })
    }

    pub fn is_alias_root(&self) -> bool {
        self.bucket.is_none()
    }

    pub fn is_bucket_root(&self) -> bool {
        self.bucket.is_some() && self.key.is_none()
    }

    pub fn key_with_trailing_slash(&self) -> Option<String> {
        let mut key = self.key.clone()?;
        if self.trailing_slash && !key.ends_with('/') {
            key.push('/');
        }
        Some(key)
    }

    pub fn require_bucket(&self) -> Result<&str> {
        self.bucket
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Target `{}` is missing bucket name.", self.alias))
    }

    pub fn require_object_key(&self) -> Result<String> {
        let bucket = self.require_bucket()?;
        let key = self.key_with_trailing_slash().ok_or_else(|| {
            anyhow::anyhow!("Target `{}/{}` is missing object key.", self.alias, bucket)
        })?;

        if key.is_empty() {
            bail!("Target `{}/{}` is missing object key.", self.alias, bucket);
        }

        Ok(key)
    }
}

pub fn is_valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.chars().enumerate().all(|(idx, ch)| match idx {
            0 => ch.is_ascii_alphabetic(),
            _ => ch.is_ascii_alphanumeric() || ch == '-' || ch == '_',
        })
}

fn validate_alias(alias: &str) -> Result<()> {
    if !is_valid_alias(alias) {
        bail!("Invalid alias `{alias}`.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TargetRef;

    #[test]
    fn parses_alias_only() {
        assert_eq!(
            TargetRef::parse("play").unwrap(),
            TargetRef {
                alias: "play".into(),
                bucket: None,
                key: None,
                trailing_slash: false,
            }
        );
    }

    #[test]
    fn parses_bucket_root() {
        assert_eq!(
            TargetRef::parse("play/mybucket/").unwrap(),
            TargetRef {
                alias: "play".into(),
                bucket: Some("mybucket".into()),
                key: None,
                trailing_slash: true,
            }
        );
    }

    #[test]
    fn parses_bucket_prefix() {
        assert_eq!(
            TargetRef::parse("play/mybucket/photos/2025").unwrap(),
            TargetRef {
                alias: "play".into(),
                bucket: Some("mybucket".into()),
                key: Some("photos/2025".into()),
                trailing_slash: false,
            }
        );
    }

    #[test]
    fn keeps_object_trailing_slash() {
        let target = TargetRef::parse("play/mybucket/photos/").unwrap();
        assert_eq!(target.key_with_trailing_slash().as_deref(), Some("photos/"));
    }

    #[test]
    fn rejects_bad_alias() {
        assert!(TargetRef::parse("123/nope").is_err());
    }
}
