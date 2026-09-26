//! `mx mv`: a `cp` session that removes each source after it was copied successfully.

use crate::commands::cp::{CopyOptions, run_session};
use crate::flags::{ChecksumFlag, EncFlags, MetadataFlags, TimeFilterFlags};
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct MoveArgs {
    /// move recursively
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub time: TimeFilterFlags,
    #[command(flatten)]
    pub metadata: MetadataFlags,
    /// preserve filesystem attributes (mode, ownership, timestamps)
    #[arg(short = 'a', long)]
    pub preserve: bool,
    /// disable multipart upload feature
    #[arg(long)]
    pub disable_multipart: bool,
    #[command(flatten)]
    pub checksum: ChecksumFlag,
    #[command(flatten)]
    pub enc: EncFlags,
    /// SOURCE [SOURCE...] TARGET
    #[arg(required = true, num_args = 2.., value_name = "PATH")]
    pub paths: Vec<String>,
}

pub fn run(args: MoveArgs, json: bool) -> Result<()> {
    let options = CopyOptions {
        recursive: args.recursive,
        time: args.time,
        metadata: args.metadata,
        preserve: args.preserve,
        disable_multipart: args.disable_multipart,
        checksum: args.checksum.checksum,
        enc: args.enc.entries()?,
        max_workers: 4,
        is_move: true,
        ..Default::default()
    };
    run_session(&args.paths, options, json)
}
