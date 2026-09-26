//! `mx idp` (mc `idp`): identity provider configuration.
//!
//! Owner: IDP.

pub mod ldap;
pub mod openid;

use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct IdpArgs {
    #[command(subcommand)]
    pub command: IdpCommand,
}

#[derive(Debug, Subcommand)]
pub enum IdpCommand {
    #[command(about = "manage OpenID IDP server configuration")]
    Openid(openid::OpenidArgs),
    #[command(about = "manage Ldap IDP server configuration")]
    Ldap(ldap::LdapArgs),
}

pub fn run(args: IdpArgs, json: bool) -> Result<()> {
    match args.command {
        IdpCommand::Openid(args) => openid::run(args, json),
        IdpCommand::Ldap(args) => ldap::run(args, json),
    }
}
