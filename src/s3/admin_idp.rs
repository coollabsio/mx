//! MinIO admin API: OpenID/LDAP IDP configuration, LDAP policy mappings and access keys (IDP area).
//!
//! Mirrors madmin-go (`idp-commands.go`, `user-commands.go`): request paths, JSON field names and
//! the client-side validation errors madmin returns before sending.

use super::admin::{AdminClient, Response, check_status, encode_component};
use crate::error::McError;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// madmin `OpenidIDPCfg`.
pub const OPENID: &str = "openid";
/// madmin `LDAPIDPCfg`.
pub const LDAP: &str = "ldap";
/// madmin `Default` config name.
pub const DEFAULT_NAME: &str = "_";

// ---------------------------------------------------------------------------
// IDP configuration
// ---------------------------------------------------------------------------

/// madmin `IDPCfgInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdpCfgInfo {
    pub key: String,
    pub value: String,
    #[serde(rename = "isCfg")]
    pub is_cfg: bool,
    #[serde(rename = "isEnv")]
    pub is_env: bool,
}

/// madmin `IDPConfig` (`info` is a Go slice: `null` when absent).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdpConfig {
    #[serde(rename = "type")]
    pub cfg_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub info: Option<Vec<IdpCfgInfo>>,
}

/// madmin `IDPListItem`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdpListItem {
    #[serde(rename = "type")]
    pub cfg_type: String,
    pub name: String,
    pub enabled: bool,
    #[serde(rename = "roleARN", skip_serializing_if = "String::is_empty")]
    pub role_arn: String,
}

/// `idp-config/<type>[/<name>]`, falling back to the query form older servers expect
/// (`426 Upgrade Required`) like madmin.
async fn idp_config_request(
    client: &AdminClient,
    method: &str,
    cfg_type: &str,
    name: Option<&str>,
    body: Option<&[u8]>,
) -> Result<Response> {
    let build = |api: &str, query: bool| -> Result<super::admin::AdminRequest<'_>> {
        let mut request = client.request(method, api);
        if query {
            request = request.query("type", cfg_type);
            if let Some(name) = name {
                request = request.query("name", name);
            }
        }
        if let Some(body) = body {
            request = request
                .header("content-type", "application/octet-stream")
                .body(body.to_vec())
                .encrypted()?;
        }
        Ok(request)
    };
    let api = match name {
        Some(name) => format!("idp-config/{cfg_type}/{name}"),
        None => format!("idp-config/{cfg_type}"),
    };
    let mut response = build(&api, false)?.send_unchecked().await?;
    if response.status == 426 {
        response = build("idp-config", true)?.send_unchecked().await?;
    }
    check_status(response)
}

/// Whether the server needs a restart (madmin: `x-minio-config-applied` is not `true`).
fn needs_restart(response: &Response) -> bool {
    response.header("x-minio-config-applied").as_deref() != Some("true")
}

fn name_or_default(name: &str) -> &str {
    if name.is_empty() { DEFAULT_NAME } else { name }
}

/// madmin `AddOrUpdateIDPConfig`: returns whether a restart is required.
pub async fn add_or_update_idp_config(
    client: &AdminClient,
    cfg_type: &str,
    name: &str,
    data: &str,
    update: bool,
) -> Result<bool> {
    let method = if update { "POST" } else { "PUT" };
    let name = name_or_default(name);
    let response =
        idp_config_request(client, method, cfg_type, Some(name), Some(data.as_bytes())).await?;
    Ok(needs_restart(&response))
}

/// madmin `GetIDPConfig`.
pub async fn get_idp_config(client: &AdminClient, cfg_type: &str, name: &str) -> Result<IdpConfig> {
    let name = name_or_default(name);
    let response = idp_config_request(client, "GET", cfg_type, Some(name), None).await?;
    Ok(serde_json::from_slice(&client.decrypt(&response.body)?)?)
}

/// madmin `ListIDPConfig` (`None` for a JSON `null`).
pub async fn list_idp_config(
    client: &AdminClient,
    cfg_type: &str,
) -> Result<Option<Vec<IdpListItem>>> {
    let response = idp_config_request(client, "GET", cfg_type, None, None).await?;
    Ok(serde_json::from_slice(&client.decrypt(&response.body)?)?)
}

/// madmin `DeleteIDPConfig`: returns whether a restart is required.
pub async fn delete_idp_config(client: &AdminClient, cfg_type: &str, name: &str) -> Result<bool> {
    let name = name_or_default(name);
    let response = idp_config_request(client, "DELETE", cfg_type, Some(name), None).await?;
    Ok(needs_restart(&response))
}

// ---------------------------------------------------------------------------
// LDAP policy mappings
// ---------------------------------------------------------------------------

/// madmin `PolicyAssociationReq`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PolicyAssociationReq {
    pub policies: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
}

impl PolicyAssociationReq {
    /// madmin `PolicyAssociationReq.IsValid`.
    pub fn validate(&self) -> std::result::Result<(), McError> {
        if self.policies.is_empty() {
            return Err(McError::new("no policy names were given"));
        }
        if self.policies.iter().any(String::is_empty) {
            return Err(McError::new("an empty policy name was given"));
        }
        if self.user.is_empty() && self.group.is_empty() {
            return Err(McError::new("no user or group association was given"));
        }
        if !self.user.is_empty() && !self.group.is_empty() {
            return Err(McError::new(
                "either a group or a user association must be given, not both",
            ));
        }
        Ok(())
    }
}

/// madmin `PolicyAssociationResp`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PolicyAssociationResp {
    #[serde(rename = "policiesAttached")]
    pub policies_attached: Option<Vec<String>>,
    #[serde(rename = "policiesDetached")]
    pub policies_detached: Option<Vec<String>>,
}

/// madmin `AttachPolicyLDAP` / `DetachPolicyLDAP`.
pub async fn ldap_policy_association(
    client: &AdminClient,
    attach: bool,
    req: &PolicyAssociationReq,
) -> Result<PolicyAssociationResp> {
    let api = if attach {
        "idp/ldap/policy/attach"
    } else {
        "idp/ldap/policy/detach"
    };
    client
        .request("POST", api)
        .header("content-type", "application/octet-stream")
        .encrypted_json(req)?
        .decrypt()
        .send_json()
        .await
}

/// madmin `GroupPolicyEntities`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupPolicyEntities {
    pub group: String,
    pub policies: Option<Vec<String>>,
}

/// madmin `UserPolicyEntities`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPolicyEntities {
    pub user: String,
    pub policies: Option<Vec<String>>,
    #[serde(rename = "memberOfMappings", skip_serializing_if = "vec_is_empty")]
    pub member_of_mappings: Option<Vec<GroupPolicyEntities>>,
}

/// madmin `PolicyEntities`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyEntities {
    pub policy: String,
    pub users: Option<Vec<String>>,
    pub groups: Option<Vec<String>>,
}

/// madmin `PolicyEntitiesResult`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyEntitiesResult {
    pub timestamp: String,
    #[serde(rename = "userMappings", skip_serializing_if = "vec_is_empty")]
    pub user_mappings: Option<Vec<UserPolicyEntities>>,
    #[serde(rename = "groupMappings", skip_serializing_if = "vec_is_empty")]
    pub group_mappings: Option<Vec<GroupPolicyEntities>>,
    #[serde(rename = "policyMappings", skip_serializing_if = "vec_is_empty")]
    pub policy_mappings: Option<Vec<PolicyEntities>>,
}

/// Values of a repeated query key in SigV4 canonical order. MinIO canonicalizes the query
/// with Go's `url.Values.Encode` (keys sorted, values in request order) while the signer
/// sorts values too: the two only agree when the values are sent sorted. The server sorts
/// its results, so the request order does not show in the output.
fn signing_order(values: &[String]) -> Vec<&str> {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_by_cached_key(|value| encode_component(value));
    sorted
}

/// Go `omitempty` for a slice: nil or empty.
fn vec_is_empty<T>(value: &Option<Vec<T>>) -> bool {
    value.as_ref().is_none_or(Vec::is_empty)
}

/// madmin `GetLDAPPolicyEntities`.
pub async fn ldap_policy_entities(
    client: &AdminClient,
    users: &[String],
    groups: &[String],
    policies: &[String],
) -> Result<PolicyEntitiesResult> {
    let mut request = client.request("GET", "idp/ldap/policy-entities");
    for (key, values) in [("group", groups), ("policy", policies), ("user", users)] {
        for value in signing_order(values) {
            request = request.query(key, value);
        }
    }
    request.decrypt().send_json().await
}

// ---------------------------------------------------------------------------
// Access keys
// ---------------------------------------------------------------------------

/// madmin `ServiceAccountInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceAccountInfo {
    #[serde(rename = "parentUser")]
    pub parent_user: String,
    #[serde(rename = "accountStatus")]
    pub account_status: String,
    #[serde(rename = "impliedPolicy")]
    pub implied_policy: bool,
    #[serde(rename = "accessKey")]
    pub access_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<String>,
}

/// madmin `ListAccessKeysResp` (`ListAccessKeysLDAPResp`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ListAccessKeysResp {
    #[serde(rename = "serviceAccounts")]
    pub service_accounts: Option<Vec<ServiceAccountInfo>>,
    #[serde(rename = "stsKeys")]
    pub sts_keys: Option<Vec<ServiceAccountInfo>>,
}

/// madmin `OpenIDUserAccessKeys`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenIdUserAccessKeys {
    #[serde(rename = "minioAccessKey")]
    pub minio_access_key: String,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "readableName")]
    pub readable_name: String,
    #[serde(rename = "serviceAccounts")]
    pub service_accounts: Option<Vec<ServiceAccountInfo>>,
    #[serde(rename = "stsKeys")]
    pub sts_keys: Option<Vec<ServiceAccountInfo>>,
}

/// madmin `ListAccessKeysOpenIDResp`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ListAccessKeysOpenIdResp {
    #[serde(rename = "configName")]
    pub config_name: String,
    pub users: Option<Vec<OpenIdUserAccessKeys>>,
}

/// madmin `AccessKeyList*` values.
pub const LIST_USERS_ONLY: &str = "users-only";
pub const LIST_STS_ONLY: &str = "sts-only";
pub const LIST_SVCACC_ONLY: &str = "svcacc-only";
pub const LIST_ALL: &str = "all";

/// madmin `ListAccessKeysOpts`.
#[derive(Debug, Clone, Default)]
pub struct ListAccessKeysOpts {
    pub list_type: String,
    pub all: bool,
    pub config_name: String,
    pub all_configs: bool,
}

/// madmin `ListAccessKeysLDAPBulkWithOpts` (sorted by DN; Go's map order is random).
pub async fn list_access_keys_ldap_bulk(
    client: &AdminClient,
    users: &[String],
    opts: &ListAccessKeysOpts,
) -> Result<BTreeMap<String, ListAccessKeysResp>> {
    if !users.is_empty() && opts.all {
        return Err(McError::new("either specify userDNs or all, not both").into());
    }
    let mut request = client
        .request("GET", "idp/ldap/list-access-keys-bulk")
        .query("listType", opts.list_type.as_str());
    for user in signing_order(users) {
        request = request.query("userDNs", user);
    }
    if opts.all {
        request = request.query("all", "true");
    }
    request.decrypt().send_json().await
}

/// madmin `ListAccessKeysOpenIDBulk`.
pub async fn list_access_keys_openid_bulk(
    client: &AdminClient,
    users: &[String],
    opts: &ListAccessKeysOpts,
) -> Result<Vec<ListAccessKeysOpenIdResp>> {
    if opts.all && !users.is_empty() {
        return Err(McError::new("either specify users or all, not both").into());
    }
    if !opts.config_name.is_empty() && opts.all_configs {
        return Err(McError::new("configName and allConfigs are mutually exclusive").into());
    }
    let mut request = client
        .request("GET", "idp/openid/list-access-keys-bulk")
        .query("listType", opts.list_type.as_str());
    for user in signing_order(users) {
        request = request.query("users", user);
    }
    if opts.all {
        request = request.query("all", "true");
    }
    if !opts.config_name.is_empty() {
        request = request.query("configName", opts.config_name.as_str());
    }
    if opts.all_configs {
        request = request.query("allConfigs", "true");
    }
    let list: Option<Vec<ListAccessKeysOpenIdResp>> = request.decrypt().send_json().await?;
    Ok(list.unwrap_or_default())
}

/// madmin `LDAPSpecificAccessKeyInfo`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LdapSpecificInfo {
    pub username: String,
}

/// madmin `OpenIDSpecificAccessKeyInfo`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct OpenIdSpecificInfo {
    #[serde(rename = "configName")]
    pub config_name: String,
    #[serde(rename = "userID")]
    pub user_id: String,
    #[serde(rename = "userIDClaim")]
    pub user_id_claim: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "displayNameClaim")]
    pub display_name_claim: String,
}

/// madmin `InfoAccessKeyResp`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct InfoAccessKeyResp {
    #[serde(rename = "parentUser")]
    pub parent_user: String,
    #[serde(rename = "accountStatus")]
    pub account_status: String,
    #[serde(rename = "impliedPolicy")]
    pub implied_policy: bool,
    pub policy: String,
    pub name: String,
    pub description: String,
    pub expiration: Option<String>,
    #[serde(rename = "userProvider")]
    pub user_provider: String,
    #[serde(rename = "ldapSpecificInfo")]
    pub ldap_specific_info: LdapSpecificInfo,
    #[serde(rename = "openIDSpecificInfo")]
    pub openid_specific_info: OpenIdSpecificInfo,
}

/// madmin `InfoAccessKey`.
pub async fn info_access_key(client: &AdminClient, access_key: &str) -> Result<InfoAccessKeyResp> {
    client
        .request("GET", "info-access-key")
        .query("accessKey", access_key)
        .decrypt()
        .send_json()
        .await
}

/// madmin `DeleteServiceAccount`.
pub async fn delete_service_account(client: &AdminClient, access_key: &str) -> Result<()> {
    client
        .request("DELETE", "delete-service-account")
        .query("accessKey", access_key)
        .send()
        .await?;
    Ok(())
}

/// madmin `UpdateServiceAccountReq`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateServiceAccountReq {
    #[serde(rename = "newPolicy", skip_serializing_if = "Option::is_none")]
    pub new_policy: Option<serde_json::Value>,
    #[serde(rename = "newSecretKey", skip_serializing_if = "String::is_empty")]
    pub new_secret_key: String,
    #[serde(rename = "newStatus", skip_serializing_if = "String::is_empty")]
    pub new_status: String,
    #[serde(rename = "newName", skip_serializing_if = "String::is_empty")]
    pub new_name: String,
    #[serde(rename = "newDescription", skip_serializing_if = "String::is_empty")]
    pub new_description: String,
    #[serde(rename = "newExpiration", skip_serializing_if = "Option::is_none")]
    pub new_expiration: Option<String>,
}

/// madmin `UpdateServiceAccount`.
pub async fn update_service_account(
    client: &AdminClient,
    access_key: &str,
    req: &UpdateServiceAccountReq,
) -> Result<()> {
    validate_service_account(
        &req.new_name,
        req.new_expiration.as_deref(),
        &req.new_description,
    )?;
    client
        .request("POST", "update-service-account")
        .query("accessKey", access_key)
        .encrypted_json(req)?
        .send()
        .await?;
    Ok(())
}

/// madmin `AddServiceAccountReq`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AddServiceAccountReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
    #[serde(rename = "targetUser", skip_serializing_if = "String::is_empty")]
    pub target_user: String,
    #[serde(rename = "accessKey", skip_serializing_if = "String::is_empty")]
    pub access_key: String,
    #[serde(rename = "secretKey", skip_serializing_if = "String::is_empty")]
    pub secret_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<String>,
}

/// madmin `Credentials` (`expiration` is a Go `time.Time`: always present).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Credentials {
    #[serde(rename = "accessKey")]
    pub access_key: String,
    #[serde(rename = "secretKey")]
    pub secret_key: String,
    #[serde(rename = "sessionToken")]
    pub session_token: String,
    pub expiration: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AddServiceAccountResp {
    credentials: Credentials,
}

/// madmin `AddServiceAccountLDAP`.
pub async fn add_service_account_ldap(
    client: &AdminClient,
    req: &AddServiceAccountReq,
) -> Result<Credentials> {
    validate_service_account(&req.name, req.expiration.as_deref(), &req.description)?;
    let resp: AddServiceAccountResp = client
        .request("PUT", "idp/ldap/add-service-account")
        .encrypted_json(req)?
        .decrypt()
        .send_json()
        .await?;
    Ok(resp.credentials)
}

/// madmin `RevokeTokensReq`.
#[derive(Debug, Clone, Default)]
pub struct RevokeTokensReq {
    pub user: String,
    pub token_revoke_type: String,
    pub full_revoke: bool,
}

/// madmin `RevokeTokens` (`provider`: `builtin`, `ldap`).
pub async fn revoke_tokens(
    client: &AdminClient,
    provider: &str,
    req: &RevokeTokensReq,
) -> Result<()> {
    if !req.user.is_empty() && req.token_revoke_type.is_empty() && !req.full_revoke {
        return Err(McError::new(
            "one of TokenRevokeType or FullRevoke must be set when User is set",
        )
        .into());
    }
    if !req.token_revoke_type.is_empty() && req.full_revoke {
        return Err(McError::new(
            "only one of TokenRevokeType or FullRevoke must be set, not both",
        )
        .into());
    }
    let mut request = client
        .request("POST", &format!("revoke-tokens/{provider}"))
        .query("tokenRevokeType", req.token_revoke_type.as_str())
        .query("user", req.user.as_str());
    if req.full_revoke {
        request = request.query("fullRevoke", "true");
    }
    request.send().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Service account validation and times
// ---------------------------------------------------------------------------

/// Go's zero `time.Time` and mc/madmin `timeSentinel` (Unix epoch): "no expiry".
pub fn is_no_expiry(time: &str) -> bool {
    time == crate::s3::admin::GO_ZERO_TIME || unix_seconds(time) == Some(0)
}

/// madmin service account checks (`validateSAName`, `validateSAExpiration`,
/// `validateSADescription`).
fn validate_service_account(
    name: &str,
    expiration: Option<&str>,
    description: &str,
) -> std::result::Result<(), McError> {
    if !name.is_empty() {
        if name.len() > 32 {
            return Err(McError::new("name must not be longer than 32 characters"));
        }
        // `^[a-zA-Z][a-zA-Z0-9_-]*` used with MatchString: only the first letter matters.
        if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return Err(McError::new(
                "name must contain only ASCII letters, digits, underscores and hyphens and must start with a letter",
            ));
        }
    }
    if let Some(expiration) = expiration
        && !is_no_expiry(expiration)
        && unix_seconds(expiration).is_some_and(|secs| secs < now_seconds())
    {
        return Err(McError::new("the expiration time should be in the future"));
    }
    if description.len() > 256 {
        return Err(McError::new("description must be at most 256 bytes long"));
    }
    Ok(())
}

/// Seconds since the Unix epoch.
pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Seconds since the Unix epoch of an RFC 3339 time (offsets allowed; before 1970 negative).
pub fn unix_seconds(time: &str) -> Option<i64> {
    let parsed = aws_smithy_types::DateTime::from_str(
        time,
        aws_smithy_types::date_time::Format::DateTimeWithOffset,
    )
    .ok()?;
    Some(parsed.secs())
}

/// Nanoseconds since the Unix epoch of an RFC 3339 time.
pub fn unix_nanos(time: &str) -> Option<i128> {
    let parsed = aws_smithy_types::DateTime::from_str(
        time,
        aws_smithy_types::date_time::Format::DateTimeWithOffset,
    )
    .ok()?;
    Some(i128::from(parsed.secs()) * 1_000_000_000 + i128::from(parsed.subsec_nanos()))
}

// ---------------------------------------------------------------------------
// STS
// ---------------------------------------------------------------------------

/// minio-go `credentials.NewLDAPIdentity(...).Get()`: `AssumeRoleWithLDAPIdentity` form post
/// (unsigned) to `path`. Errors carry the STS error message.
pub async fn assume_role_with_ldap_identity(
    client: &AdminClient,
    path: &str,
    username: &str,
    password: &str,
) -> Result<Credentials> {
    let form = [
        ("Action", "AssumeRoleWithLDAPIdentity"),
        ("LDAPPassword", password),
        ("LDAPUsername", username),
        ("Version", "2011-06-15"),
    ]
    .iter()
    .map(|(k, v)| format!("{}={}", encode_component(k), encode_component(v)))
    .collect::<Vec<_>>()
    .join("&");
    let path = if path.is_empty() { "/" } else { path };
    let response = client
        .send_anonymous(
            "POST",
            path,
            &[(
                "content-type",
                "application/x-www-form-urlencoded".to_string(),
            )],
            form.into_bytes(),
        )
        .await?;
    let text = response.text();
    if response.status != 200 {
        let message = xml_text(&text, "Message")
            .filter(|m| !m.is_empty())
            .or_else(|| xml_text(&text, "Code").map(|code| format!("Error response code {code}.")))
            .unwrap_or_else(|| {
                http::StatusCode::from_u16(response.status)
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            });
        return Err(McError::new(message).into());
    }
    Ok(Credentials {
        access_key: xml_text(&text, "AccessKeyId").unwrap_or_default(),
        secret_key: xml_text(&text, "SecretAccessKey").unwrap_or_default(),
        session_token: xml_text(&text, "SessionToken").unwrap_or_default(),
        expiration: xml_text(&text, "Expiration"),
    })
}

fn xml_text(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&close)?;
    Some(text[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_association_validation_matches_madmin() {
        let mut req = PolicyAssociationReq {
            policies: vec!["p".into()],
            ..Default::default()
        };
        assert_eq!(
            req.validate().unwrap_err().message,
            "no user or group association was given"
        );
        req.user = "u".into();
        assert!(req.validate().is_ok());
        req.group = "g".into();
        assert_eq!(
            req.validate().unwrap_err().message,
            "either a group or a user association must be given, not both"
        );
        req.policies = vec![String::new()];
        assert_eq!(
            req.validate().unwrap_err().message,
            "an empty policy name was given"
        );
    }

    #[test]
    fn service_account_validation_matches_madmin() {
        assert!(validate_service_account("ok-name_1", None, "").is_ok());
        assert!(validate_service_account("a.b", None, "").is_ok());
        assert!(validate_service_account("1bad", None, "").is_err());
        assert!(validate_service_account(&"a".repeat(33), None, "").is_err());
        assert!(validate_service_account("", None, &"d".repeat(257)).is_err());
        assert_eq!(
            validate_service_account("", Some("2001-01-01T00:00:00Z"), "")
                .unwrap_err()
                .message,
            "the expiration time should be in the future"
        );
        assert!(validate_service_account("", Some("1970-01-01T00:00:00Z"), "").is_ok());
        assert!(validate_service_account("", Some("0001-01-01T00:00:00Z"), "").is_ok());
        assert!(validate_service_account("", Some("2999-01-01T00:00:00Z"), "").is_ok());
    }

    #[test]
    fn policy_entities_round_trip_keeps_go_omitempty() {
        let json = r#"{"timestamp":"2026-01-01T00:00:00.5Z","policyMappings":[{"policy":"p","users":null,"groups":["g"]}]}"#;
        let result: PolicyEntitiesResult = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&result).unwrap(), json);
    }

    #[test]
    fn service_account_info_keeps_go_fields() {
        let json = r#"{"parentUser":"","accountStatus":"","impliedPolicy":false,"accessKey":"k","expiration":"2026-09-27T19:27:08Z"}"#;
        let info: ServiceAccountInfo = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&info).unwrap(), json);
    }

    #[test]
    fn times_parse_with_offsets() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("1970-01-01T01:00:00+01:00"), Some(0));
        assert!(is_no_expiry("0001-01-01T00:00:00Z"));
        assert!(is_no_expiry("1970-01-01T00:00:00Z"));
        assert!(!is_no_expiry("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn xml_text_finds_first_tag() {
        assert_eq!(
            xml_text("<a><Message>m</Message></a>", "Message").as_deref(),
            Some("m")
        );
        assert_eq!(xml_text("<a/>", "Message"), None);
    }
}
