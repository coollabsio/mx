//! Top-level CLI: global flags and the command list. Each command's Args live in its module
//! under `crate::commands`.

use crate::commands::{
    alias, anonymous, cat, cors, cp, diff, du, encrypt, event, find, get, head, ilm, legalhold, ls,
    mb, mirror, mv, od, ping, pipe, put, quota, rb, ready, replicate, retention, rm, share, stat,
    tag, tree, undo, version,
};
use crate::resolve::ResolveMapping;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about = "MaxIO Client", long_about = None)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[arg(short = 'C', long = "config-dir", global = true, value_name = "PATH")]
    pub config_dir: Option<std::path::PathBuf>,

    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,

    #[arg(long, global = true)]
    pub insecure: bool,

    #[arg(long, global = true, value_name = "HOST:PORT=IP")]
    pub resolve: Vec<ResolveMapping>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "manage server credentials in configuration file")]
    Alias(alias::AliasArgs),
    #[command(about = "list buckets and objects")]
    Ls(ls::LsArgs),
    #[command(about = "make bucket")]
    Mb(mb::MakeBucketArgs),
    #[command(about = "remove bucket")]
    Rb(rb::RemoveBucketArgs),
    #[command(about = "show object or bucket information")]
    Stat(stat::StatArgs),
    #[command(about = "print object contents to stdout")]
    Cat(cat::CatArgs),
    #[command(about = "remove object")]
    Rm(rm::RemoveArgs),
    #[command(about = "copy objects and files")]
    Cp(cp::CopyArgs),
    #[command(about = "move object")]
    Mv(mv::MoveArgs),
    #[command(visible_alias = "out", about = "upload object")]
    Put(put::PutArgs),
    #[command(about = "mirror a directory tree")]
    Mirror(mirror::MirrorArgs),
    #[command(about = "upload standard input to an object")]
    Pipe(pipe::PipeArgs),
    #[command(about = "download an object to the local filesystem")]
    Get(get::GetArgs),
    #[command(about = "display the first lines of an object or file")]
    Head(head::HeadArgs),
    #[command(about = "summarize disk usage")]
    Du(du::DuArgs),
    #[command(about = "search for objects and files")]
    Find(find::FindArgs),
    #[command(about = "list prefixes and objects in a tree")]
    Tree(tree::TreeArgs),
    #[command(about = "list name and size differences")]
    Diff(diff::DiffArgs),
    #[command(about = "generate presigned URLs")]
    Share(share::ShareArgs),
    #[command(about = "check if a server is ready")]
    Ready(ready::HealthArgs),
    #[command(about = "ping an S3 server")]
    Ping(ping::PingArgs),
    #[command(about = "manage object and bucket tags")]
    Tag(tag::TagArgs),
    #[command(about = "manage bucket versioning")]
    Version(version::VersionArgs),
    #[command(about = "manage bucket CORS")]
    Cors(cors::CorsArgs),
    #[command(about = "manage bucket encryption")]
    Encrypt(encrypt::EncryptArgs),
    #[command(about = "manage anonymous bucket access")]
    Anonymous(anonymous::AnonymousArgs),
    #[command(about = "manage bucket lifecycle")]
    Ilm(ilm::IlmArgs),
    #[command(about = "set retention for object(s)")]
    Retention(retention::RetentionArgs),
    #[command(about = "manage legal hold for object(s)")]
    Legalhold(legalhold::LegalholdArgs),
    #[command(about = "manage object notifications")]
    Event(event::EventArgs),
    #[command(about = "undo PUT/DELETE operations")]
    Undo(undo::UndoArgs),
    #[command(about = "measure single stream upload and download")]
    Od(od::OdArgs),
    #[command(about = "configure server side bucket replication")]
    Replicate(replicate::ReplicateArgs),
    #[command(about = "manage bucket quota")]
    Quota(quota::QuotaArgs),
}
