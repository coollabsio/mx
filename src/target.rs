use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRef {
    pub alias: String,
    pub bucket: Option<String>,
    pub prefix: Option<String>,
}

impl TargetRef {
    pub fn parse(input: &str) -> Result<Self> {
        let raw = input.trim();
        if raw.is_empty() {
            bail!("Target cannot be empty.");
        }

        let normalized = raw.replace('\\', "/");
        let trimmed = normalized.trim_matches('/');
        if trimmed.is_empty() {
            bail!("Target cannot be empty.");
        }

        let mut parts = trimmed.splitn(3, '/');
        let alias = parts.next().unwrap_or_default().to_string();
        validate_alias(&alias)?;

        let bucket = parts.next().map(str::to_string);
        let prefix = parts
            .next()
            .map(str::to_string)
            .and_then(|value| if value.is_empty() { None } else { Some(value) });

        Ok(Self {
            alias,
            bucket,
            prefix,
        })
    }
}

fn validate_alias(alias: &str) -> Result<()> {
    let valid = !alias.is_empty()
        && alias.chars().enumerate().all(|(idx, ch)| match idx {
            0 => ch.is_ascii_alphabetic(),
            _ => ch.is_ascii_alphanumeric() || ch == '-' || ch == '_',
        });

    if !valid {
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
                prefix: None,
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
                prefix: None,
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
                prefix: Some("photos/2025".into()),
            }
        );
    }

    #[test]
    fn rejects_bad_alias() {
        assert!(TargetRef::parse("123/nope").is_err());
    }
}
