//! `mx sql` (mc `sql`).
//!
//! Owner: JOBS. Stubs return "not implemented yet" until implemented.

use crate::commands::not_implemented;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct SqlArgs {
    #[arg(value_name = "TARGETS", required = true)]
    pub targets: Vec<String>,
    #[arg(
        long = "query",
        short = 'e',
        default_value = "select * from s3object",
        value_name = "VALUE",
        help = "sql query expression"
    )]
    pub query: String,
    #[arg(long = "recursive", short = 'r', help = "sql query recursively")]
    pub recursive: bool,
    #[arg(
        long = "csv-input",
        value_name = "VALUE",
        help = "csv input serialization option"
    )]
    pub csv_input: Option<String>,
    #[arg(
        long = "json-input",
        value_name = "VALUE",
        help = "json input serialization option"
    )]
    pub json_input: Option<String>,
    #[arg(
        long = "compression",
        value_name = "VALUE",
        help = "input compression type"
    )]
    pub compression: Option<String>,
    #[arg(
        long = "csv-output",
        value_name = "VALUE",
        help = "csv output serialization option"
    )]
    pub csv_output: Option<String>,
    #[arg(
        long = "csv-output-header",
        value_name = "VALUE",
        help = "optional csv output header"
    )]
    pub csv_output_header: Option<String>,
    #[arg(
        long = "json-output",
        value_name = "VALUE",
        help = "json output serialization option"
    )]
    pub json_output: Option<String>,
    #[arg(
        long = "enc-c",
        value_name = "VALUE",
        help = "encrypt/decrypt objects using client provided keys. (multiple keys can be provided) Formats: RawBase64 or Hex."
    )]
    pub enc_c: Vec<String>,
}

pub fn run(args: SqlArgs, json: bool) -> Result<()> {
    let _ = (args, json);
    not_implemented("sql")
}
