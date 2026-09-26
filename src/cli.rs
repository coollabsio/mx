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
    /// enable JSON lines formatted output
    #[arg(long, global = true)]
    pub json: bool,

    /// path to configuration folder
    #[arg(short = 'C', long = "config-dir", global = true, value_name = "PATH")]
    pub config_dir: Option<std::path::PathBuf>,

    /// disable progress bar display
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,

    /// disable mc internal pager and print to raw stdout (mx has no pager)
    #[arg(long = "disable-pager", visible_alias = "dp", global = true)]
    pub disable_pager: bool,

    /// disable color theme (mx has no colors)
    #[arg(long, global = true)]
    pub no_color: bool,

    /// enable debug output
    #[arg(long, global = true)]
    pub debug: bool,

    /// disable SSL certificate verification
    #[arg(long, global = true)]
    pub insecure: bool,

    /// resolves HOST:PORT to an IP address, e.g. minio.local:9000=10.10.75.1
    #[arg(long, global = true, value_name = "HOST:PORT=IP")]
    pub resolve: Vec<ResolveMapping>,

    /// limits uploads to a maximum rate in KiB/s, MiB/s, GiB/s (default: unlimited)
    #[arg(long, global = true, value_name = "RATE", value_parser = parse_rate)]
    pub limit_upload: Option<u64>,

    /// limits downloads to a maximum rate in KiB/s, MiB/s, GiB/s (default: unlimited)
    #[arg(long, global = true, value_name = "RATE", value_parser = parse_rate)]
    pub limit_download: Option<u64>,

    /// add custom HTTP header to the request, 'key:value' format
    #[arg(
        short = 'H',
        long = "custom-header",
        global = true,
        value_name = "KEY:VALUE",
        value_parser = parse_custom_header
    )]
    pub custom_header: Vec<(String, String)>,

    #[command(subcommand)]
    pub command: Commands,
}

fn parse_rate(value: &str) -> anyhow::Result<u64> {
    crate::net::parse_bytes(value)
}

fn parse_custom_header(value: &str) -> anyhow::Result<(String, String)> {
    crate::net::parse_custom_header(value)
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
