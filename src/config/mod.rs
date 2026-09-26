pub mod env_alias;
pub mod model;

use crate::config::model::{AliasConfig, ConfigV10};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use tempfile::NamedTempFile;

static CONFIG_DIR: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

pub fn configure_dir(dir: Option<PathBuf>) {
    *CONFIG_DIR
        .get_or_init(|| RwLock::new(None))
        .write()
        .expect("config dir lock poisoned") = dir;
}

fn configured_dir() -> Option<PathBuf> {
    CONFIG_DIR
        .get_or_init(|| RwLock::new(None))
        .read()
        .expect("config dir lock poisoned")
        .clone()
}

pub const CONFIG_FILE_NAME: &str = "config.json";
const MX_DIR_NAME: &str = ".mx";
const MC_DIR_NAME: &str = ".mc";

/// Loaded config file plus the environment alias overlay.
///
/// [`ConfigStore::config`] is the merged view used for alias resolution: `MC_HOST_<alias>`
/// wins over `MC_CONFIG_ENV_FILE` aliases, which win over the config file (like mc
/// `expandAlias`). [`ConfigStore::config_mut`] and [`ConfigStore::save`] only touch the file
/// config, so environment aliases (and their secrets) are never written to disk.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    file: ConfigV10,
    config: ConfigV10,
    /// `MC_HOST_<alias>` variables that failed to parse, by alias.
    env_errors: BTreeMap<String, String>,
}

impl ConfigStore {
    pub fn load_or_create() -> Result<Self> {
        let (path, config) = Self::load_file()?;
        let mut view = config.clone();
        let file_path = path.display().to_string();
        for alias in view.aliases.values_mut() {
            alias.src = Some(file_path.clone());
        }
        for (alias, cfg) in env_alias::env_file_aliases() {
            view.aliases.insert(alias.clone(), cfg.clone());
        }
        let mut env_errors = BTreeMap::new();
        for (alias, parsed) in env_alias::host_env_aliases() {
            match parsed {
                Ok(cfg) => {
                    view.aliases.insert(alias, cfg);
                }
                Err(err) => {
                    env_errors.insert(alias, err);
                }
            }
        }
        Ok(Self {
            path,
            file: config,
            config: view,
            env_errors,
        })
    }

    fn load_file() -> Result<(PathBuf, ConfigV10)> {
        if let Some(dir) = configured_dir() {
            let path = dir.join(CONFIG_FILE_NAME);
            if path.exists() {
                return Self::load_from_path(path);
            }
            let config = ConfigV10::new_with_defaults();
            save_config(&path, &config)?;
            return Ok((path, config));
        }

        let home = home_dir()?;
        let mx_path = home.join(MX_DIR_NAME).join(CONFIG_FILE_NAME);
        let mc_path = home.join(MC_DIR_NAME).join(CONFIG_FILE_NAME);

        if mx_path.exists() {
            return Self::load_from_path(mx_path);
        }
        if mc_path.exists() {
            return Self::load_from_path(mc_path);
        }

        let config = ConfigV10::new_with_defaults();
        save_config(&mx_path, &config)?;
        Ok((mx_path, config))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Merged view (config file + environment aliases). Every alias has `src` set.
    pub fn config(&self) -> &ConfigV10 {
        &self.config
    }

    /// The config file contents only (no environment aliases). Call [`ConfigStore::save`] after
    /// changing it; [`ConfigStore::config`] is not refreshed.
    #[allow(clippy::misnamed_getters)] // intentionally the file config, not the merged view
    pub fn config_mut(&mut self) -> &mut ConfigV10 {
        &mut self.file
    }

    /// Resolves an alias like mc `expandAlias`: an invalid `MC_HOST_<alias>` is an error.
    pub fn alias(&self, alias: &str) -> Result<AliasConfig> {
        if let Some(err) = self.env_errors.get(alias) {
            bail!("{err}");
        }
        self.config
            .aliases
            .get(alias)
            .cloned()
            .ok_or_else(|| anyhow!("No such alias `{alias}` found."))
    }

    pub fn save(&self) -> Result<()> {
        save_config(&self.path, &self.file)
    }

    fn load_from_path(path: PathBuf) -> Result<(PathBuf, ConfigV10)> {
        let file = fs::File::open(&path)
            .with_context(|| format!("Unable to open config file `{}`.", path.display()))?;
        let reader = BufReader::new(file);
        let config: ConfigV10 = serde_json::from_reader(reader)
            .with_context(|| format!("Unable to parse config file `{}`.", path.display()))?;

        if config.version != model::CONFIG_VERSION {
            bail!(
                "Unsupported config version `{}` in `{}`.",
                config.version,
                path.display()
            );
        }

        Ok((path, config))
    }
}

pub fn save_config(path: &Path, config: &ConfigV10) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Invalid config path `{}`.", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Unable to create config dir `{}`.", parent.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok();
    }

    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("Unable to create temp config in `{}`.", parent.display()))?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, config)
            .with_context(|| format!("Unable to serialize config for `{}`.", path.display()))?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    temp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("Unable to persist config to `{}`.", path.display()))?;
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }
    dirs::home_dir().ok_or_else(|| anyhow!("Unable to determine home directory."))
}

/// Loads `MC_CONFIG_ENV_FILE` aliases (if the variable is set). Call once at startup.
pub fn load_env_config_file() -> Result<()> {
    env_alias::load_env_file()
}
