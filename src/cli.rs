use crate::resolve::ResolveMapping;
use clap::{Args, Parser, Subcommand};

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
    Alias(AliasArgs),
    #[command(about = "list buckets and objects")]
    Ls(LsArgs),
    #[command(about = "make bucket")]
    Mb(MakeBucketArgs),
    #[command(about = "remove bucket")]
    Rb(RemoveBucketArgs),
    #[command(about = "show object or bucket information")]
    Stat(TargetArg),
    #[command(about = "print object contents to stdout")]
    Cat(TargetArg),
    #[command(about = "remove object")]
    Rm(RemoveArgs),
    #[command(about = "copy objects and files")]
    Cp(CopyArgs),
    #[command(about = "move object")]
    Mv(CopyArgs),
    #[command(visible_alias = "out", about = "upload object")]
    Put(PutArgs),
    #[command(about = "mirror a directory tree")]
    Mirror(MirrorArgs),
    #[command(about = "upload standard input to an object")]
    Pipe(PipeArgs),
    #[command(about = "download an object to the local filesystem")]
    Get(GetArgs),
    #[command(about = "display the first lines of an object or file")]
    Head(HeadArgs),
    #[command(about = "summarize disk usage")]
    Du(DuArgs),
    #[command(about = "search for objects and files")]
    Find(FindArgs),
    #[command(about = "list prefixes and objects in a tree")]
    Tree(TreeArgs),
    #[command(about = "list name and size differences")]
    Diff(DiffArgs),
    #[command(about = "generate presigned URLs")]
    Share(ShareArgs),
    #[command(about = "check if a server is ready")]
    Ready(HealthArgs),
    #[command(about = "ping an S3 server")]
    Ping(PingArgs),
    #[command(about = "manage object and bucket tags")]
    Tag(TagArgs),
    #[command(about = "manage bucket versioning")]
    Version(VersionArgs),
    #[command(about = "manage bucket CORS")]
    Cors(CorsArgs),
    #[command(about = "manage bucket encryption")]
    Encrypt(EncryptArgs),
    #[command(about = "manage anonymous bucket access")]
    Anonymous(AnonymousArgs),
    #[command(about = "manage bucket lifecycle rules")]
    Ilm(IlmArgs),
}

#[derive(Debug, Args)]
pub struct AliasArgs {
    #[command(subcommand)]
    pub command: AliasCommand,
}

#[derive(Debug, Subcommand)]
pub enum AliasCommand {
    #[command(visible_alias = "s", about = "set a new alias to configuration file")]
    Set(AliasSetArgs),
    #[command(visible_alias = "ls", about = "list aliases in configuration file")]
    List(AliasListArgs),
    #[command(
        visible_alias = "rm",
        about = "remove an alias from configuration file"
    )]
    Remove(AliasRemoveArgs),
    #[command(about = "import an alias from JSON")]
    Import(AliasImportArgs),
    #[command(about = "export an alias as JSON")]
    Export(AliasExportArgs),
}

#[derive(Debug, Args)]
pub struct AliasSetArgs {
    pub alias: String,
    pub url: String,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    #[arg(long, default_value = "S3v4")]
    pub api: String,
    #[arg(long, default_value = "auto")]
    pub path: String,
}

#[derive(Debug, Args)]
pub struct AliasListArgs {
    pub alias: Option<String>,
}

#[derive(Debug, Args)]
pub struct AliasRemoveArgs {
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct AliasImportArgs {
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct AliasExportArgs {
    pub alias: String,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    #[arg(short = 'r', long)]
    pub recursive: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct BucketTargetArgs {
    pub target: String,
}

#[derive(Debug, Args)]
pub struct MakeBucketArgs {
    #[arg(long)]
    pub ignore_existing: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct RemoveBucketArgs {
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub dangerous: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct TargetArg {
    pub target: String,
}

#[derive(Debug, Args)]
pub struct CopyArgs {
    #[arg(short = 'r', long)]
    pub recursive: bool,
    pub source: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[arg(long)]
    pub force: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct PutArgs {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct PipeArgs {
    #[arg(long)]
    pub quiet: bool,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct MirrorArgs {
    #[arg(long)]
    pub remove: bool,
    pub source: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct GetArgs {
    pub source: String,
    pub target: Option<String>,
}

#[derive(Debug, Args)]
pub struct HeadArgs {
    #[arg(short = 'n', long, default_value_t = 10)]
    pub lines: usize,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct DuArgs {
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[arg(short = 'd', long)]
    pub depth: Option<usize>,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct FindArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub regex: Option<String>,
    #[arg(long)]
    pub maxdepth: Option<usize>,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    #[arg(short = 'f', long)]
    pub files: bool,
    #[arg(short = 'd', long)]
    pub depth: Option<usize>,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct DiffArgs {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ShareArgs {
    #[command(subcommand)]
    pub command: ShareCommand,
}

#[derive(Debug, Subcommand)]
pub enum ShareCommand {
    #[command(about = "generate a presigned download URL")]
    Download(ShareUrlArgs),
    #[command(about = "generate a presigned upload URL")]
    Upload(ShareUrlArgs),
    #[command(about = "list generated share URLs")]
    List,
}

#[derive(Debug, Args)]
pub struct ShareUrlArgs {
    #[arg(short = 'E', long, default_value = "168h")]
    pub expire: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct HealthArgs {
    pub target: String,
}

#[derive(Debug, Args)]
pub struct PingArgs {
    #[arg(short = 'c', long, default_value_t = 4)]
    pub count: u32,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub command: TagCommand,
}

#[derive(Debug, Subcommand)]
pub enum TagCommand {
    #[command(about = "set tags")]
    Set(TagSetArgs),
    #[command(about = "list tags")]
    List(TargetArg),
    #[command(about = "remove tags")]
    Remove(TargetArg),
}

#[derive(Debug, Args)]
pub struct TagSetArgs {
    pub target: String,
    pub tags: String,
}

#[derive(Debug, Args)]
pub struct VersionArgs {
    #[command(subcommand)]
    pub command: VersionCommand,
}

#[derive(Debug, Subcommand)]
pub enum VersionCommand {
    #[command(about = "enable bucket versioning")]
    Enable(TargetArg),
    #[command(about = "suspend bucket versioning")]
    Suspend(TargetArg),
    #[command(about = "show bucket versioning")]
    Info(TargetArg),
}

#[derive(Debug, Args)]
pub struct CorsArgs {
    #[command(subcommand)]
    pub command: CorsCommand,
}

#[derive(Debug, Subcommand)]
pub enum CorsCommand {
    #[command(about = "set bucket CORS from a JSON file")]
    Set(CorsSetArgs),
    #[command(about = "show bucket CORS")]
    Get(TargetArg),
    #[command(about = "remove bucket CORS")]
    Remove(TargetArg),
}

#[derive(Debug, Args)]
pub struct CorsSetArgs {
    pub target: String,
    pub file: String,
}

#[derive(Debug, Args)]
pub struct EncryptArgs {
    #[command(subcommand)]
    pub command: EncryptCommand,
}

#[derive(Debug, Subcommand)]
pub enum EncryptCommand {
    #[command(about = "set default bucket encryption")]
    Set(EncryptSetArgs),
    #[command(about = "show bucket encryption")]
    Info(TargetArg),
    #[command(about = "clear bucket encryption")]
    Clear(TargetArg),
}

#[derive(Debug, Args)]
pub struct EncryptSetArgs {
    pub algorithm: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct AnonymousArgs {
    #[command(subcommand)]
    pub command: AnonymousCommand,
}

#[derive(Debug, Subcommand)]
pub enum AnonymousCommand {
    #[command(about = "set anonymous access policy")]
    Set(AnonymousSetArgs),
    #[command(about = "show anonymous access policy")]
    Get(TargetArg),
}

#[derive(Debug, Args)]
pub struct AnonymousSetArgs {
    #[arg(help = "download, upload, public, or none")]
    pub policy: String,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct IlmArgs {
    #[command(subcommand)]
    pub command: IlmCommand,
}

#[derive(Debug, Subcommand)]
pub enum IlmCommand {
    #[command(about = "manage lifecycle rules")]
    Rule(IlmRuleArgs),
}

#[derive(Debug, Args)]
pub struct IlmRuleArgs {
    #[command(subcommand)]
    pub command: IlmRuleCommand,
}

#[derive(Debug, Subcommand)]
pub enum IlmRuleCommand {
    #[command(about = "add a lifecycle rule")]
    Add(IlmRuleAddArgs),
    #[command(about = "list lifecycle rules")]
    List(TargetArg),
    #[command(about = "remove a lifecycle rule")]
    Remove(IlmRuleRemoveArgs),
}

#[derive(Debug, Args)]
pub struct IlmRuleAddArgs {
    #[arg(long)]
    pub expire_days: Option<i32>,
    #[arg(long)]
    pub prefix: Option<String>,
    #[arg(long)]
    pub id: Option<String>,
    pub target: String,
}

#[derive(Debug, Args)]
pub struct IlmRuleRemoveArgs {
    #[arg(long)]
    pub id: String,
    pub target: String,
}
