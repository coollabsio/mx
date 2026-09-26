use crate::commands::cp::CopyArgs;
use crate::config::ConfigStore;
use crate::location::{Location, parse_location};
use anyhow::{Result, bail};
use clap::Args;

#[derive(Debug, Args)]
pub struct MirrorArgs {
    #[arg(long)]
    pub remove: bool,
    pub source: String,
    pub target: String,
}

pub fn run(args: MirrorArgs, json: bool) -> Result<()> {
    let store = ConfigStore::load_or_create()?;
    let source = parse_location(&args.source, store.config());
    let target = parse_location(&args.target, store.config());

    if matches!((&source, &target), (Location::Local(_), Location::Local(_))) {
        bail!("Local-to-local mirror is not supported by `mx mirror`.");
    }
    if args.remove {
        bail!("`mirror --remove` is not implemented yet.");
    }
    if !matches!((&source, &target), (Location::Local(path), Location::S3(_)) if path.is_dir()) {
        bail!("This preview supports only local-directory-to-S3 mirror.");
    }

    super::cp::run(
        CopyArgs {
            recursive: true,
            paths: vec![args.source, args.target],
            ..Default::default()
        },
        json,
    )
}
